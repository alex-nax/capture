// audiocap.swift — capture audio from a single macOS application via
// ScreenCaptureKit and stream it as raw 16 kHz mono signed-16-bit-LE PCM on
// stdout, so a parent process can pipe it into an ASR backend.
//
// Build:
//   swiftc -O -o audiocap audiocap.swift -framework ScreenCaptureKit \
//          -framework AVFoundation -framework CoreMedia
//
// Usage:
//   audiocap --pid <PID> [--rate 16000]
//   audiocap --bundle <bundle.id> [--rate 16000]
//
// stdout : raw PCM (s16le, mono, <rate> Hz) — pipe this.
// stderr : human-readable status; first line is "READY rate=<n> channels=1 fmt=s16le".
//
// Requires the Screen Recording permission (System Settings ▸ Privacy &
// Security ▸ Screen Recording) for the process that launches this helper.

import AVFoundation
import CoreMedia
import Foundation
import ScreenCaptureKit

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

if targetPID == nil && targetBundle == nil && !captureSystem {
    FileHandle.standardError.write("usage: audiocap --pid <PID> | --bundle <id> | --system [--rate 16000]\n".data(using: .utf8)!)
    exit(2)
}

func logErr(_ s: String) {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
}

// ---- stream output handler --------------------------------------------------

final class AudioSink: NSObject, SCStreamOutput, SCStreamDelegate {
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

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sampleBuffer.isValid else { return }
        guard let pcm = makeInputBuffer(from: sampleBuffer) else { return }

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
        shutdown(1)
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

Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        logErr("content: apps=\(content.applications.count) displays=\(content.displays.count) windows=\(content.windows.count)")
        guard let display = content.displays.first else {
            logErr("no display available")
            exit(4)
        }

        let filter: SCContentFilter
        let label: String
        if captureSystem {
            filter = SCContentFilter(display: display, excludingWindows: [])
            label = "system"
        } else {
            guard let app = findApp(in: content) else {
                logErr("no running application matched pid=\(targetPID.map(String.init) ?? "-") bundle=\(targetBundle ?? "-")")
                exit(3)
            }
            // Filter the whole display down to just this app's audio.
            filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
            label = "\(app.applicationName) pid=\(app.processID)"
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

        logErr("target=\(label); starting capture...")
        let stream = SCStream(filter: filter, configuration: config, delegate: sink)
        captureStream = stream  // retain for the process lifetime
        try stream.addStreamOutput(sink, type: .audio, sampleHandlerQueue: audioQueue)
        try await stream.startCapture()

        logErr("READY rate=\(Int(targetRate)) channels=1 fmt=s16le target=\(label)")

        // Stop cleanly on SIGINT/SIGTERM from the parent. The sources are kept
        // alive in the global `signalSources` (a local would be cancelled when
        // this Task returns and the handler would never fire).
        let stop: () -> Void = {
            Task {
                try? await stream.stopCapture()
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
    } catch {
        logErr("startup failed: \(error.localizedDescription)")
        exit(5)
    }
}

RunLoop.main.run()
