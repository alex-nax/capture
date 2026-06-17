// audiocap.swift — capture audio from a single macOS application via
// ScreenCaptureKit and stream it as raw 16 kHz mono signed-16-bit-LE PCM on
// stdout, so a parent process can pipe it into an ASR backend.
//
// Build:
//   swiftc -O -o audiocap audiocap.swift -framework ScreenCaptureKit \
//          -framework AVFoundation -framework CoreMedia \
//          -framework ImageIO -framework UniformTypeIdentifiers
//
// Usage:
//   audiocap --pid <PID> [--rate 16000]
//   audiocap --bundle <bundle.id> [--rate 16000]
//   audiocap --mic [<deviceUniqueID>] [--rate 16000]   (microphone via AVCaptureSession)
//   audiocap --list-mics                                (print input devices as JSON lines, exit)
//   audiocap --extract-audio <file> [--rate 16000]      (decode a file's audio → s16le on stdout)
//   audiocap --extract-frames <file> --out <dir> [--interval 2.0]  (write <offset_ms>.png frames)
//
// The two --extract-* modes are offline file readers for `capture import` (turn an
// existing audio/video file into a session). They use AVFoundation only — no
// ScreenCaptureKit, no capture, no ffmpeg — and need no permission. --extract-audio
// emits the SAME s16le contract as the live paths; --extract-frames writes one PNG
// per sampled timestamp named by its integer millisecond offset (audio-only files
// produce zero frames and exit 0).
//
// The --mic path uses AVFoundation AVCaptureSession (NOT ScreenCaptureKit), so it
// needs only the Microphone permission — no Screen Recording — and no ffmpeg. It
// emits the SAME s16le PCM + READY contract as the app path, so the parent treats
// app audio and mic audio identically (capture-mcp records the mic as a separate
// track: mic.s16le / mic_transcript.jsonl). NOTE: no echo cancellation — a laptop's
// built-in mic will pick up its own speakers; use headphones. (System voice-processing
// AEC was tried but it ducks/mutes other apps' audio — tracked as a feature, #38.)
//
// stdout : raw PCM (s16le, mono, <rate> Hz) — pipe this.
// stderr : human-readable status. Diagnostics (content counts, target) come
//          first; "READY rate=<n> channels=1 fmt=s16le target=..." is emitted
//          once startCapture succeeds (and again on reconnects). Parents must
//          SCAN stderr lines for the READY prefix, not read line 1.
//          Frozen protocol: docs/specs/helper-contract.md.
//
// Requires the Screen Recording permission (System Settings ▸ Privacy &
// Security ▸ Screen Recording) for the process that launches this helper.

import AVFoundation
import CoreMedia
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

// ---- argument parsing -------------------------------------------------------

func argValue(_ name: String) -> String? {
    let a = CommandLine.arguments
    guard let i = a.firstIndex(of: name), i + 1 < a.count else { return nil }
    return a[i + 1]
}

let targetPID = argValue("--pid").flatMap { Int32($0) }
let targetBundle = argValue("--bundle")
let targetRate = Double(argValue("--rate") ?? "16000") ?? 16000
let captureSystem = CommandLine.arguments.contains("--system")  // whole-display audio
let listMics = CommandLine.arguments.contains("--list-mics")
let micMode = CommandLine.arguments.contains("--mic")
// The value after --mic is the device uniqueID; absent / another flag / "default"
// all mean "system default input device".
let micDeviceID: String? = {
    guard let v = argValue("--mic"), !v.hasPrefix("--"), v != "default" else { return nil }
    return v
}()

// Offline import: pull a file's audio/frames through AVFoundation (no ffmpeg, no
// capture). --extract-audio writes s16le to stdout (same contract as the live
// paths); --extract-frames writes <offset_ms>.png files into --out.
let extractAudioPath = argValue("--extract-audio")
let extractFramesPath = argValue("--extract-frames")
let extractInterval = Double(argValue("--interval") ?? "2.0") ?? 2.0
let extractOutDir = argValue("--out")

if targetPID == nil && targetBundle == nil && !captureSystem && !micMode && !listMics
    && extractAudioPath == nil && extractFramesPath == nil {
    FileHandle.standardError.write("usage: audiocap --pid <PID> | --bundle <id> | --system | --mic [<id>] | --list-mics\n              | --extract-audio <file> | --extract-frames <file> --out <dir> [--interval 2.0] [--rate 16000]\n".data(using: .utf8)!)
    exit(2)
}

func logErr(_ s: String) {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
}

// ---- microphone device listing (--list-mics) --------------------------------
// One JSON object per stdout line: {"id","name","default"}. Used by the daemon's
// GET /v1/audio/mics to populate the GUI's mic selector. AVFoundation only — no
// ScreenCaptureKit, no ffmpeg, needs no permission to enumerate.

func listMicrophonesAndExit() -> Never {
    // `.microphone` covers built-in and USB/external audio inputs; we deliberately
    // omit `.external` (Continuity-Camera iPhone mics, plus a deprecation warning).
    let discovery = AVCaptureDevice.DiscoverySession(
        deviceTypes: [.microphone],
        mediaType: .audio,
        position: .unspecified
    )
    let defaultID = AVCaptureDevice.default(for: .audio)?.uniqueID
    for d in discovery.devices {
        let obj: [String: Any] = ["id": d.uniqueID, "name": d.localizedName, "default": d.uniqueID == defaultID]
        if let data = try? JSONSerialization.data(withJSONObject: obj),
           let line = String(data: data, encoding: .utf8) {
            print(line)
        }
    }
    exit(0)
}

if listMics { listMicrophonesAndExit() }

// ---- offline import (--extract-audio / --extract-frames) --------------------
// AVFoundation-only file readers used by `capture import`. No capture, no ffmpeg.
// AVAssetReader decodes + resamples to our s16le contract; AVAssetImageGenerator
// pulls frames. Both load tracks via the modern async API (the sync accessors are
// deprecated and can return stale/empty results), bridged to this synchronous CLI
// with a semaphore.

func loadFirstTrack(_ asset: AVAsset, _ media: AVMediaType) -> AVAssetTrack? {
    let sem = DispatchSemaphore(value: 0)
    var track: AVAssetTrack?
    Task {
        track = try? await asset.loadTracks(withMediaType: media).first
        sem.signal()
    }
    sem.wait()
    return track
}

func loadDuration(_ asset: AVAsset) -> Double {
    let sem = DispatchSemaphore(value: 0)
    var seconds = 0.0
    Task {
        if let d = try? await asset.load(.duration) { seconds = d.seconds }
        sem.signal()
    }
    sem.wait()
    return seconds.isFinite ? seconds : 0
}

// Exit codes for --extract-audio: 0 ok, 3 = no audio track (recoverable — a silent
// video still imports as a frames-only session), 4 = genuine open/read failure.
func extractAudioAndExit(path: String, rate: Double) -> Never {
    let asset = AVURLAsset(url: URL(fileURLWithPath: path))
    guard let track = loadFirstTrack(asset, .audio) else {
        logErr("extract-audio: no audio track in \(path)")
        exit(3)
    }
    let settings: [String: Any] = [
        AVFormatIDKey: kAudioFormatLinearPCM,
        AVSampleRateKey: rate,
        AVNumberOfChannelsKey: 1,
        AVLinearPCMBitDepthKey: 16,
        AVLinearPCMIsFloatKey: false,
        AVLinearPCMIsBigEndianKey: false,
        AVLinearPCMIsNonInterleaved: false,
    ]
    guard let reader = try? AVAssetReader(asset: asset) else {
        logErr("extract-audio: cannot open \(path)")
        exit(4)
    }
    let output = AVAssetReaderTrackOutput(track: track, outputSettings: settings)
    output.alwaysCopiesSampleData = false
    guard reader.canAdd(output) else { logErr("extract-audio: cannot add reader output"); exit(4) }
    reader.add(output)
    guard reader.startReading() else {
        logErr("extract-audio: startReading failed: \(reader.error?.localizedDescription ?? "?")")
        exit(4)
    }
    let out = FileHandle.standardOutput
    var total = 0
    while reader.status == .reading {
        guard let sb = output.copyNextSampleBuffer() else { break }
        if let bb = CMSampleBufferGetDataBuffer(sb) {
            var len = 0
            var dataPtr: UnsafeMutablePointer<Int8>?
            if CMBlockBufferGetDataPointer(bb, atOffset: 0, lengthAtOffsetOut: nil,
                                           totalLengthOut: &len, dataPointerOut: &dataPtr) == kCMBlockBufferNoErr,
               let dp = dataPtr {
                out.write(Data(bytes: dp, count: len))
                total += len
            }
        }
        CMSampleBufferInvalidate(sb)
    }
    if reader.status == .failed {
        logErr("extract-audio: read failed: \(reader.error?.localizedDescription ?? "?")")
        exit(4)
    }
    logErr("extract-audio: wrote \(total) bytes s16le @ \(Int(rate))Hz mono")
    exit(0)
}

func extractFramesAndExit(path: String, interval: Double, outDir: String?) -> Never {
    guard let dir = outDir else { logErr("extract-frames: --out <dir> required"); exit(2) }
    let asset = AVURLAsset(url: URL(fileURLWithPath: path))
    let duration = loadDuration(asset)
    // Audio-only files (or zero-length) yield no frames — that's not an error; the
    // import just becomes an audio-only session.
    guard loadFirstTrack(asset, .video) != nil, duration > 0 else {
        logErr("extract-frames: no video track in \(path) — 0 frames")
        exit(0)
    }
    try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    let gen = AVAssetImageGenerator(asset: asset)
    gen.appliesPreferredTrackTransform = true
    gen.requestedTimeToleranceBefore = .zero
    gen.requestedTimeToleranceAfter = .zero
    gen.maximumSize = CGSize(width: 1920, height: 1080)  // cap 4K frames to a screenshot-sized PNG
    let step = interval > 0 ? interval : 2.0
    var t = 0.0
    var count = 0
    while t <= duration {
        let cmt = CMTime(seconds: t, preferredTimescale: 600)
        if let cg = try? gen.copyCGImage(at: cmt, actualTime: nil) {
            let ms = Int((t * 1000).rounded())
            let fileURL = URL(fileURLWithPath: dir).appendingPathComponent("\(ms).png")
            if let dest = CGImageDestinationCreateWithURL(fileURL as CFURL, UTType.png.identifier as CFString, 1, nil) {
                CGImageDestinationAddImage(dest, cg, nil)
                if CGImageDestinationFinalize(dest) { count += 1 }
            }
        }
        t += step
    }
    logErr("extract-frames: wrote \(count) frames at \(step)s interval")
    exit(0)
}

if let p = extractAudioPath { extractAudioAndExit(path: p, rate: targetRate) }
if let p = extractFramesPath { extractFramesAndExit(path: p, interval: extractInterval, outDir: extractOutDir) }

// ---- stream output handler --------------------------------------------------

final class AudioSink: NSObject, SCStreamOutput, SCStreamDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
    let outRate: Double
    var converter: AVAudioConverter?
    var outFormat: AVAudioFormat?
    let stdout = FileHandle.standardOutput

    init(outRate: Double) {
        self.outRate = outRate
        self.outFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: outRate,
            channels: 1,
            interleaved: true
        )
    }

    // ScreenCaptureKit app/system audio.
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sampleBuffer.isValid else { return }
        guard let pcm = makeInputBuffer(from: sampleBuffer) else { return }
        convertAndWrite(pcm)
    }

    // AVCaptureSession microphone audio (the --mic path). Same conversion + output
    // contract as the ScreenCaptureKit path above, so the parent can't tell them apart.
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard sampleBuffer.isValid, let pcm = makeInputBuffer(from: sampleBuffer) else { return }
        convertAndWrite(pcm)
    }

    // Convert an audio PCM buffer to s16le @ outRate mono and write it to stdout.
    func convertAndWrite(_ pcm: AVAudioPCMBuffer) {
        // Lazily build the converter from the real input buffer's format. The
        // converter instance is retained across callbacks, so any resampler tail
        // carries into the next buffer; only the final buffer's tail at stream
        // end can be lost (negligible). We request 16 kHz from SCStream, so this
        // is usually a pure Float32->Int16 conversion with no resampling at all.
        if converter == nil {
            guard let outFormat else { return }
            if pcm.format.sampleRate != outFormat.sampleRate {
                logErr("note: input rate \(pcm.format.sampleRate) != \(outFormat.sampleRate); resampling")
            }
            converter = AVAudioConverter(from: pcm.format, to: outFormat)
        }
        guard let converter, let outFormat else { return }

        let ratio = outRate / pcm.format.sampleRate
        let capacity = AVAudioFrameCount(Double(pcm.frameLength) * ratio + 1024)
        guard let outBuf = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return }

        var supplied = false
        var err: NSError?
        converter.convert(to: outBuf, error: &err) { _, status in
            if supplied {
                status.pointee = .noDataNow
                return nil
            }
            supplied = true
            status.pointee = .haveData
            return pcm
        }
        if let err {
            logErr("convert error: \(err.localizedDescription)")
            return
        }
        writeInt16(outBuf)
    }

    // Copy a ScreenCaptureKit audio CMSampleBuffer into an AVAudioPCMBuffer.
    private func makeInputBuffer(from sampleBuffer: CMSampleBuffer) -> AVAudioPCMBuffer? {
        guard let fmtDesc = sampleBuffer.formatDescription,
              let asbd = fmtDesc.audioStreamBasicDescription else { return nil }
        var asbdVar = asbd
        guard let avFormat = AVAudioFormat(streamDescription: &asbdVar) else { return nil }
        let frames = AVAudioFrameCount(sampleBuffer.numSamples)
        guard frames > 0,
              let buf = AVAudioPCMBuffer(pcmFormat: avFormat, frameCapacity: frames) else { return nil }
        buf.frameLength = frames

        let abl = buf.mutableAudioBufferList
        let status = CMSampleBufferCopyPCMDataIntoAudioBufferList(
            sampleBuffer, at: 0, frameCount: Int32(frames), into: abl)
        if status != noErr {
            logErr("CMSampleBufferCopyPCMDataIntoAudioBufferList: \(status)")
            return nil
        }
        return buf
    }

    private func writeInt16(_ buf: AVAudioPCMBuffer) {
        guard let ch = buf.int16ChannelData, buf.frameLength > 0 else { return }
        let n = Int(buf.frameLength)
        let data = Data(bytes: ch[0], count: n * MemoryLayout<Int16>.size)
        if !everGotData { everGotData = true; logErr("audio flowing") }
        reconnects = 0  // a healthy stream resets the backoff
        // SIGPIPE is ignored at startup, so a broken pipe surfaces as a throw
        // here (parent went away) rather than killing us with an exception.
        do {
            try stdout.write(contentsOf: data)
        } catch {
            logErr("stdout closed (\(error.localizedDescription)); exiting")
            shutdown(0)
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        let ns = error as NSError
        logErr("stream stopped: \(error.localizedDescription) [domain=\(ns.domain) code=\(ns.code)]")
        if stopping { return }
        // -3801 userDeclined / -3803 missingEntitlements are genuine permission
        // failures — retrying is pointless, so report and exit.
        if ns.code == -3801 || ns.code == -3803 {
            logErr("permission error — grant Screen Recording (see README); not retrying")
            shutdown(1)
        }
        // Otherwise (e.g. -3805 connection interrupted on a Space/focus change),
        // rebuild the stream and keep capturing in the background.
        scheduleReconnect()
    }
}

// ---- wiring -----------------------------------------------------------------

func findApp(in content: SCShareableContent) -> SCRunningApplication? {
    if let pid = targetPID {
        return content.applications.first { $0.processID == pid }
    }
    if let bundle = targetBundle {
        return content.applications.first { $0.bundleIdentifier == bundle }
    }
    return nil
}

// Ignore SIGPIPE so a closed stdout surfaces as a throwing write (handled in
// writeInt16) rather than killing the process with an unhandled signal.
signal(SIGPIPE, SIG_IGN)

let audioQueue = DispatchQueue(label: "audiocap.audio")
var signalSources: [DispatchSourceSignal] = []  // retained for the process lifetime
var captureStream: SCStream?  // global so ARC never releases it mid-capture
var micSession: AVCaptureSession?  // retained for the --mic path's process lifetime

// Terminations can be requested concurrently (EPIPE on the audio queue, a signal
// handler, the stream-error delegate). Funnel them so exactly one thread exits.
let shutdownLock = NSLock()
var didShutdown = false
func shutdown(_ code: Int32) -> Never {
    shutdownLock.lock()
    if didShutdown {
        shutdownLock.unlock()
        while true { Thread.sleep(forTimeInterval: 3600) }  // another thread is exiting
    }
    didShutdown = true
    shutdownLock.unlock()
    exit(code)
}

let sink = AudioSink(outRate: targetRate)

// Reconnection state. A stream connection can be interrupted (SCStreamError -3805)
// when Spaces/displays/app focus change while capturing in the background — which
// is exactly when the user is doing other things. Instead of dying, we rebuild the
// stream and keep going. The filter/config are built once and reused per attempt.
var capFilter: SCContentFilter?
var capConfig: SCStreamConfiguration?
var capLabel = ""
var stopping = false
var reconnects = 0
var everGotData = false

func scheduleReconnect() {
    if stopping { return }
    reconnects += 1
    // If we never managed to get any audio, don't spin forever — give up after a
    // bounded number of attempts (~30s with backoff) and report.
    if !everGotData && reconnects > 20 {
        logErr("giving up after \(reconnects) failed connection attempts with no audio")
        shutdown(1)
    }
    let delay = min(2.0, 0.25 * Double(reconnects))  // backoff, capped at 2s
    DispatchQueue.main.asyncAfter(deadline: .now() + delay) { connect() }
}

func connect() {
    if stopping { return }
    guard let filter = capFilter, let config = capConfig else { return }
    Task {
        do {
            let stream = SCStream(filter: filter, configuration: config, delegate: sink)
            captureStream = stream  // retain so ARC can't release it mid-capture
            try stream.addStreamOutput(sink, type: .audio, sampleHandlerQueue: audioQueue)
            try await stream.startCapture()
            logErr("READY rate=\(Int(targetRate)) channels=1 fmt=s16le target=\(capLabel)"
                   + (reconnects > 0 ? " (reconnect #\(reconnects))" : ""))
        } catch {
            let ns = error as NSError
            logErr("startCapture failed: \(error.localizedDescription) [code=\(ns.code)]; retrying")
            scheduleReconnect()
        }
    }
}

// Enumerate shareable content with a bounded retry. On macOS 26 the first
// SCShareableContent call intermittently fails; a single attempt would exit(5)
// and lean entirely on the parent's respawn (observed in the #30 TCC spike).
// Retry a few times before giving up.
func enumerateShareableContent() async -> SCShareableContent? {
    for attempt in 1...5 {
        do {
            return try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        } catch {
            logErr("shareable content enumeration failed (attempt \(attempt)/5): \(error.localizedDescription)")
            if attempt < 5 { try? await Task.sleep(nanoseconds: 500_000_000) }
        }
    }
    return nil
}

// ---- microphone capture (--mic) ---------------------------------------------
// AVFoundation AVCaptureSession on the chosen (or default) input device. Emits the
// same s16le PCM + READY contract as the ScreenCaptureKit path. Needs the Microphone
// permission only (no Screen Recording). NOTE: no echo cancellation — see the header
// (system voice-processing AEC ducks other apps' audio; tracked as feature #38).

func startMicCapture() {
    let device: AVCaptureDevice? =
        (micDeviceID.flatMap { AVCaptureDevice(uniqueID: $0) }) ?? AVCaptureDevice.default(for: .audio)
    guard let device else { logErr("no audio input device available"); shutdown(3) }
    guard let input = try? AVCaptureDeviceInput(device: device) else {
        logErr("cannot open microphone '\(device.localizedName)' — check Microphone permission")
        shutdown(3)
    }
    let session = AVCaptureSession()
    guard session.canAddInput(input) else { logErr("cannot add mic input"); shutdown(3) }
    session.addInput(input)
    let output = AVCaptureAudioDataOutput()
    output.setSampleBufferDelegate(sink, queue: audioQueue)
    guard session.canAddOutput(output) else { logErr("cannot add audio data output"); shutdown(3) }
    session.addOutput(output)
    micSession = session
    // Stop cleanly on SIGINT/SIGTERM.
    let stop: () -> Void = { stopping = true; micSession?.stopRunning(); shutdown(0) }
    for s in [SIGTERM, SIGINT] {
        signal(s, SIG_IGN)
        let src = DispatchSource.makeSignalSource(signal: s, queue: .main)
        src.setEventHandler(handler: stop)
        src.resume()
        signalSources.append(src)
    }
    session.startRunning()
    logErr("READY rate=\(Int(targetRate)) channels=1 fmt=s16le target=mic:\(device.localizedName)")
}

if micMode { startMicCapture() }

if !micMode { Task {
        guard let content = await enumerateShareableContent() else {
            logErr("startup failed: could not enumerate shareable content after 5 attempts")
            exit(5)
        }
        logErr("content: apps=\(content.applications.count) displays=\(content.displays.count) windows=\(content.windows.count)")
        guard let display = content.displays.first else {
            logErr("no display available")
            exit(4)
        }

        if captureSystem {
            capFilter = SCContentFilter(display: display, excludingWindows: [])
            capLabel = "system"
        } else {
            guard let app = findApp(in: content) else {
                logErr("no running application matched pid=\(targetPID.map(String.init) ?? "-") bundle=\(targetBundle ?? "-")")
                exit(3)
            }
            // Filter the whole display down to just this app's audio.
            capFilter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
            capLabel = "\(app.applicationName) pid=\(app.processID)"
        }

        let config = SCStreamConfiguration()
        config.capturesAudio = true
        config.sampleRate = Int(targetRate)
        config.channelCount = 1
        config.excludesCurrentProcessAudio = true
        // We only want audio, but a valid (non-degenerate) video config is still
        // required; very small sizes are rejected on recent macOS.
        config.width = 128
        config.height = 128
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        config.queueDepth = 6
        capConfig = config

        // Stop cleanly on SIGINT/SIGTERM. Set `stopping` first so an in-flight
        // stream error doesn't trigger a reconnect during shutdown.
        let stop: () -> Void = {
            stopping = true
            Task {
                try? await captureStream?.stopCapture()
                shutdown(0)  // FileHandle writes are unbuffered, so nothing to drain
            }
        }
        for s in [SIGTERM, SIGINT] {
            signal(s, SIG_IGN)
            let src = DispatchSource.makeSignalSource(signal: s, queue: .main)
            src.setEventHandler(handler: stop)
            src.resume()
            signalSources.append(src)
        }

        logErr("target=\(capLabel); starting capture...")
        connect()
    }
}

RunLoop.main.run()
