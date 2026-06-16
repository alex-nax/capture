// CaptureBar — the macOS menu-bar agent for capture-mcp.
//
// A thin, always-resident NSStatusItem app (LSUIElement, no Dock icon) that is the
// bundle's entry point. It owns the things that must outlive the GPUI window:
//   * the persistent menu-bar icon (reflects daemon/capture state),
//   * the daemon lifecycle (spawns the bundled `captured`; stops it on Quit),
//   * launching the GPUI window (`capture-gui`) on demand.
//
// It is a peer client of the daemon like everything else: it reads
// ~/.capture/daemon.json for the endpoint + bearer token and polls /v1. No capture
// logic lives here. A sibling Windows agent is planned (see docs/specs/agent.md).
//
// Build: swiftc -O -o CaptureBar CaptureBar.swift   (links AppKit)

import AppKit
import AVFoundation
import Foundation

// MARK: - Daemon client (mirrors daemon/client.py discovery + a couple of routes)

struct DaemonInfo {
    let endpoint: String
    let token: String
}

enum Daemon {
    /// ~/.capture/daemon.json (or $CAPTURE_DAEMON_JSON), if a daemon wrote one.
    static func discover() -> DaemonInfo? {
        let path = ProcessInfo.processInfo.environment["CAPTURE_DAEMON_JSON"]
            .map { ($0 as NSString).expandingTildeInPath }
            ?? (NSHomeDirectory() as NSString).appendingPathComponent(".capture/daemon.json")
        guard let data = FileManager.default.contents(atPath: path),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let endpoint = obj["endpoint"] as? String,
              let token = obj["token"] as? String
        else { return nil }
        return DaemonInfo(endpoint: endpoint, token: token)
    }

    /// Synchronous GET/POST helper (call off the main thread). Returns parsed JSON.
    static func request(
        _ method: String, _ info: DaemonInfo, _ route: String,
        body: [String: Any]? = nil, timeout: TimeInterval = 4
    ) -> [String: Any]? {
        guard let url = URL(string: info.endpoint + route) else { return nil }
        var req = URLRequest(url: url, timeoutInterval: timeout)
        req.httpMethod = method
        req.setValue("Bearer \(info.token)", forHTTPHeaderField: "Authorization")
        if let body = body {
            req.httpBody = try? JSONSerialization.data(withJSONObject: body)
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let sem = DispatchSemaphore(value: 0)
        var out: [String: Any]?
        let task = URLSession.shared.dataTask(with: req) { data, _, _ in
            defer { sem.signal() }
            if let data = data {
                out = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            }
        }
        task.resume()
        _ = sem.wait(timeout: .now() + timeout + 1)
        return out
    }
}

/// Snapshot of daemon state the menu renders from.
struct State {
    var daemonUp = false
    var runningCaptures = 0
    var totalSessions = 0
}

// MARK: - Agent

final class Agent: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem!
    private var timer: Timer?
    private var state = State()
    /// True once WE spawned the daemon — governs whether Quit stops it.
    private var weStartedDaemon = false
    /// Set by an explicit "Stop Daemon" — suppresses auto-respawn until "Start Daemon".
    private var userStoppedDaemon = false
    /// When we last spawned the daemon (debounces auto-respawn during startup).
    private var lastSpawn: Date?
    /// The GUI window process we launched (if still running, focus it vs. relaunch).
    private var guiProcess: Process?

    // Menu items we mutate as state changes.
    private let headerItem = NSMenuItem(title: "capture", action: nil, keyEquivalent: "")
    private let stopAllItem = NSMenuItem(title: "Stop All Captures", action: nil, keyEquivalent: "")
    private let daemonItem = NSMenuItem(title: "Stop Daemon", action: nil, keyEquivalent: "")

    func applicationDidFinishLaunching(_ note: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        applyIcon(symbol: "record.circle", count: 0)  // visible from the moment we launch
        buildMenu()
        ensureDaemon()            // start the bundled daemon if none is running
        openWindow(self)          // first launch: show the window once
        poll()                    // immediate first refresh
        timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            self?.poll()
        }
    }

    // MARK: menu

    private func buildMenu() {
        let menu = NSMenu()
        headerItem.isEnabled = false
        menu.addItem(headerItem)
        menu.addItem(.separator())

        let open = NSMenuItem(title: "Open Window", action: #selector(openWindow(_:)), keyEquivalent: "o")
        open.target = self
        menu.addItem(open)

        stopAllItem.action = #selector(stopAll(_:))
        stopAllItem.target = self
        menu.addItem(stopAllItem)
        menu.addItem(.separator())

        daemonItem.action = #selector(toggleDaemon(_:))
        daemonItem.target = self
        menu.addItem(daemonItem)
        menu.addItem(.separator())

        let quit = NSMenuItem(title: "Quit Capture", action: #selector(quit(_:)), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)

        statusItem.menu = menu
    }

    /// Set the menu-bar icon (an SF Symbol template image — far more visible than text)
    /// plus an optional running-capture count. Falls back to text if the symbol is missing.
    private func applyIcon(symbol: String, count: Int) {
        guard let button = statusItem.button else { return }
        if let img = NSImage(systemSymbolName: symbol, accessibilityDescription: "Capture") {
            img.isTemplate = true
            button.image = img
            button.imagePosition = count > 0 ? .imageLeading : .imageOnly
            button.title = count > 0 ? " \(count)" : ""
        } else {
            button.image = nil
            button.title = count > 0 ? "⦿ \(count)" : "● capture"
        }
    }

    /// Reflect `state` into the status-item icon + menu labels (main thread).
    private func render() {
        applyIcon(
            symbol: state.runningCaptures > 0 ? "record.circle.fill" : "record.circle",
            count: state.runningCaptures
        )

        if !state.daemonUp {
            headerItem.title = "daemon: stopped"
        } else if state.runningCaptures > 0 {
            headerItem.title = "daemon: running · \(state.runningCaptures) capturing"
        } else {
            headerItem.title = "daemon: running · idle"
        }
        stopAllItem.isEnabled = state.runningCaptures > 0
        daemonItem.title = state.daemonUp ? "Stop Daemon" : "Start Daemon"
    }

    // MARK: polling

    private func poll() {
        DispatchQueue.global(qos: .utility).async {
            var s = State()
            if let info = Daemon.discover(),
               let health = Daemon.request("GET", info, "/v1/health", timeout: 2),
               health["ok"] as? Bool == true {
                s.daemonUp = true
                if let sessions = Daemon.request("GET", info, "/v1/sessions"),
                   let list = sessions["sessions"] as? [[String: Any]] {
                    s.totalSessions = list.count
                    s.runningCaptures = list.filter { ($0["state"] as? String) == "running" }.count
                }
            }
            DispatchQueue.main.async {
                self.state = s
                self.render()
                // Auto-respawn the daemon if it went away — crash recovery, and what
                // makes the GUI's "Restart daemon" work (shut down → respawn, so a new
                // Screen Recording grant takes effect). Suppressed only by an explicit
                // "Stop Daemon" (so it isn't fought); robust to however it first started.
                if !s.daemonUp && !self.userStoppedDaemon {
                    self.ensureDaemon()
                }
            }
        }
    }

    // MARK: daemon lifecycle

    /// Path to the bundled frozen daemon: Contents/Resources/captured/captured.
    private func bundledDaemon() -> URL? {
        let url = Bundle.main.resourceURL?.appendingPathComponent("captured/captured")
        return url.flatMap { FileManager.default.isExecutableFile(atPath: $0.path) ? $0 : nil }
    }

    /// Start the bundled daemon iff none is answering. Detached so it isn't killed
    /// if the agent is force-quit (a normal Quit stops it gracefully via /v1).
    private func ensureDaemon() {
        if let info = Daemon.discover(),
           Daemon.request("GET", info, "/v1/health", timeout: 1)?["ok"] as? Bool == true {
            return // already running (maybe started by the CLI / a previous launch)
        }
        // Debounce: a freshly spawned daemon takes ~1–2 s to answer; don't pile up
        // spawns (the 2 s poll would otherwise re-spawn during that startup window).
        if let last = lastSpawn, Date().timeIntervalSince(last) < 6 {
            return
        }
        guard let bin = bundledDaemon() else { return }
        let p = Process()
        p.executableURL = bin
        p.standardInput = FileHandle.nullDevice
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        do {
            try p.run()
            lastSpawn = Date()
            weStartedDaemon = true
        } catch {
            NSLog("CaptureBar: failed to start bundled daemon: \(error)")
        }
    }

    private func shutdownDaemon() {
        guard let info = Daemon.discover() else { return }
        _ = Daemon.request("POST", info, "/v1/admin/shutdown", body: [:], timeout: 3)
    }

    // MARK: actions

    /// Launch the GPUI window helper (a sibling binary in Contents/MacOS). We tell it
    /// (CAPTURE_AGENT=1) to skip its own tray + daemon-spawn and to exit on window close.
    @objc private func openWindow(_ sender: Any?) {
        // A window is already open (its process is still running) → focus it instead
        // of launching a duplicate.
        if let p = guiProcess, p.isRunning {
            NSRunningApplication(processIdentifier: p.processIdentifier)?
                .activate(options: [.activateAllWindows])
            return
        }
        guard let dir = Bundle.main.executableURL?.deletingLastPathComponent() else { return }
        let gui = dir.appendingPathComponent("capture-gui")
        guard FileManager.default.isExecutableFile(atPath: gui.path) else {
            NSLog("CaptureBar: capture-gui not found at \(gui.path)")
            return
        }
        let p = Process()
        p.executableURL = gui
        var env = ProcessInfo.processInfo.environment
        env["CAPTURE_AGENT"] = "1"
        p.environment = env
        do {
            try p.run()
            guiProcess = p  // track it so the next "Open Window" focuses, not relaunches
        } catch {
            NSLog("CaptureBar: failed to launch capture-gui: \(error)")
        }
    }

    @objc private func stopAll(_ sender: Any?) {
        DispatchQueue.global(qos: .userInitiated).async {
            guard let info = Daemon.discover(),
                  let sessions = Daemon.request("GET", info, "/v1/sessions"),
                  let list = sessions["sessions"] as? [[String: Any]] else { return }
            for s in list where (s["state"] as? String) == "running" {
                if let id = s["session_id"] as? String {
                    _ = Daemon.request("POST", info, "/v1/sessions/\(id)/stop", body: [:])
                }
            }
            DispatchQueue.main.async { self.poll() }
        }
    }

    @objc private func toggleDaemon(_ sender: Any?) {
        if state.daemonUp {
            userStoppedDaemon = true  // suppress auto-respawn until "Start Daemon"
            DispatchQueue.global(qos: .userInitiated).async {
                self.shutdownDaemon()
                self.weStartedDaemon = false
                DispatchQueue.main.async { self.poll() }
            }
        } else {
            userStoppedDaemon = false
            ensureDaemon()
            poll()
        }
    }

    @objc private func quit(_ sender: Any?) {
        // Stop the daemon we own so its binary stops pinning the .app (lets the user
        // delete/replace it) — unless captures are actively running.
        if state.daemonUp && state.runningCaptures == 0 {
            shutdownDaemon()
        }
        NSApp.terminate(nil)
    }
}

// MARK: - main

// One-shot mode: the GUI's Microphone "Grant" spawns `CaptureBar --request-mic`. The
// daemon can't prompt for mic (it aborts headless), so this Swift one-shot does it —
// and since every binary shares the bundle's Team ID, the grant reaches the daemon.
// We wait for the user's answer (the prompt is system-modal) before exiting.
if CommandLine.arguments.contains("--request-mic") {
    let sem = DispatchSemaphore(value: 0)
    AVCaptureDevice.requestAccess(for: .audio) { _ in sem.signal() }
    _ = sem.wait(timeout: .now() + 120)
    exit(0)
}

let app = NSApplication.shared
let agent = Agent()
app.delegate = agent
app.setActivationPolicy(.accessory) // menu-bar only; no Dock icon (also LSUIElement in Info.plist)
app.run()
