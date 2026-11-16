// Exercise the real compiled frontend in the separate synthetic native fixture.
// This is UI evidence only, not a VM, OS credential-store or inference walk.
import AppKit
import CoreGraphics
import ApplicationServices
import Vision

let display = CGMainDisplayID()
var previousDisplayMode: CGDisplayMode?
func restoreDisplay() {
    if let previous = previousDisplayMode {
        previousDisplayMode = nil
        if CGDisplaySetDisplayMode(display, previous, nil) != .success {
            fputs("Cannot restore CI display mode\n", stderr)
        }
    }
}
func require(_ value: Bool, _ message: String) {
    if !value { fputs("\(message)\n", stderr); restoreDisplay(); exit(1) }
}
// The preceding native walk restores the hosted Mac's small default display.
// Enlarge only that disposable CI display for the local 540x900 result captures.
if CGDisplayBounds(display).width < 1280 || CGDisplayBounds(display).height < 1080 {
    require(ProcessInfo.processInfo.environment["GITHUB_ACTIONS"] == "true", "Local result captures need a 1280x1080 CI display")
    let options = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
    let modes = CGDisplayCopyAllDisplayModes(display, options) as? [CGDisplayMode] ?? []
    guard let previous = CGDisplayCopyDisplayMode(display),
        let larger = modes.filter({ $0.isUsableForDesktopGUI() && $0.width >= 1280 && $0.height >= 1080 })
            .min(by: { $0.width * $0.height < $1.width * $1.height }) else { fatalError("No suitable CI display mode") }
    previousDisplayMode = previous
    require(CGDisplaySetDisplayMode(display, larger, nil) == .success, "Cannot enlarge CI display")
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 1))
}
defer { restoreDisplay() }
let output = CommandLine.arguments[1]
let theme = CommandLine.arguments[3]
require(["light", "dark"].contains(theme), "Expected light or dark")
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
let child = Process()
child.executableURL = URL(fileURLWithPath: CommandLine.arguments[2])
let keyCheckRelease = URL(fileURLWithPath: output).appendingPathComponent("key-check-release-\(UUID().uuidString)")
let setupProgress = URL(fileURLWithPath: output).appendingPathComponent("setup-progress-\(UUID().uuidString)")
let catalogRelease = URL(fileURLWithPath: output).appendingPathComponent("catalog-release-\(UUID().uuidString)")
defer { try? FileManager.default.removeItem(at: catalogRelease) }
// Written only for the fresh-profile first-test outcomes; absent keeps the fixed failed report.
let firstTestOutcome = URL(fileURLWithPath: output).appendingPathComponent("first-test-outcome-\(UUID().uuidString)")
defer { try? FileManager.default.removeItem(at: firstTestOutcome) }
try "installing".write(to: setupProgress, atomically: true, encoding: .utf8)
defer { try? FileManager.default.removeItem(at: setupProgress) }
child.environment = ProcessInfo.processInfo.environment.merging(["BRIDGE_MODEL_CHECK_RELEASE": keyCheckRelease.path, "BRIDGE_SETUP_PROGRESS": setupProgress.path, "BRIDGE_FIRST_TEST_OUTCOME": firstTestOutcome.path, "BRIDGE_CATALOG_RELEASE": catalogRelease.path]) { _, fixture in fixture }
try child.run()
defer { if child.isRunning { child.terminate(); child.waitUntilExit() } }
func window() -> (CGWindowID, CGRect)? {
    let list = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
    guard let w = list.first(where: {
        ($0[kCGWindowOwnerPID as String] as? Int32) == child.processIdentifier &&
        ($0[kCGWindowLayer as String] as? Int) == 0
    }), let id = w[kCGWindowNumber as String] as? UInt32,
        let bounds = w[kCGWindowBounds as String] as? [String: Any],
        let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return nil }
    return (id, rect)
}
for _ in 0..<30 {
    if window() != nil { break }
    Thread.sleep(forTimeInterval: 1)
}
require(window() != nil, "Native model fixture did not open")
NSRunningApplication(processIdentifier: child.processIdentifier)?.activate(options: [])
Thread.sleep(forTimeInterval: 2)
func capture(_ name: String, _ expected: [String]) throws -> [VNRecognizedTextObservation] {
    let file = "\(output)/\(name).png"
    let command = Process()
    command.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    command.arguments = ["-x", "-o", "-l", String(window()!.0), file]
    try command.run(); command.waitUntilExit()
    require(command.terminationStatus == 0, "Cannot capture native model UI")
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    try VNImageRequestHandler(url: URL(fileURLWithPath: file)).perform([request])
    let rows = request.results ?? []
    let text = rows.compactMap { $0.topCandidates(1).first?.string }.joined(separator: "\n")
    try text.write(toFile: "\(output)/\(name).txt", atomically: true, encoding: .utf8)
    // Vision can interleave the adjacent setup column between wrapped lines.
    // Check phrases within the model panel, not the whole window's reading order.
    let visible: String
    if name.hasPrefix("local-") || name.hasPrefix("chat-") {
        visible = rows.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
    } else {
        guard let panel = rows.filter({
            let label = $0.topCandidates(1).first?.string ?? ""
            // The shell now also says Connect the AI. Anchor below the tab bar, in the model column.
            return (label.contains("Connect the AI") || label.contains("Connect the Al")) && $0.boundingBox.minX > 0.5 && $0.boundingBox.maxY < 0.85
        }).max(by: { $0.boundingBox.maxY < $1.boundingBox.maxY })
            else { fatalError("Model panel heading was not recognized") }
        visible = rows.filter {
            $0.boundingBox.minX >= panel.boundingBox.minX - 0.02 &&
            $0.boundingBox.maxY <= panel.boundingBox.maxY + 0.01
        }.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
    }
    for value in expected {
        // Vision renders the capital I as a lowercase l in this exact observed label.
        let found = visible.replacingOccurrences(of: "Connect the Al", with: "Connect the AI")
            .replacingOccurrences(of: "APl key", with: "API key")
            .replacingOccurrences(of: "Reset Al connection", with: "Reset AI connection").contains(value)
        // Recovery copy has apostrophes and "AI" that Vision misreads; require visible native text instead.
        let lenient = ["local-key-", "local-reset-", "local-remove-"].contains { name.hasPrefix($0) }
        let nativeText = !found && name.hasPrefix("local-") && (nativeResultTextIsVisible(value) || lenient && nativeChatTextIsVisible(value))
        let chatText = !found && name.hasPrefix("chat-") && nativeChatTextIsVisible(value)
        let workspaceControl = !found && name.hasPrefix("local-workspace-") && nativeWorkspaceButtonIsVisible(value)
        require(found || nativeText || chatText || workspaceControl, "Missing \(value) in \(name): \(text)")
    }
    guard let bitmap = NSBitmapImageRep(data: try Data(contentsOf: URL(fileURLWithPath: file))),
          let colour = bitmap.colorAt(x: bitmap.pixelsWide - 20, y: bitmap.pixelsHigh - 20)?.usingColorSpace(.deviceRGB)
        else { fatalError("Cannot sample native theme") }
    let brightness = (colour.redComponent + colour.greenComponent + colour.blueComponent) / 3
    require(theme == "dark" ? brightness < 0.25 : brightness > 0.8, "Wrong system theme")
    // Setup frames 1c, 1d (cropped) and 1e exist only in light; dark captures use them as layout references.
    // Frames 1g (AI connection failed) and 1h (AI reset) are approved in both themes.
    let frame: (file: String, label: String)? =
        name.hasPrefix("local-workspace-") || name == "local-stopped" ? ("workspace-reference-\(theme)", "Approved workspace frame (wide reference)")
        : ["local-result", "local-answer", "local-story", "local-running", "local-stopping", "local-shell"].contains(where: { name.hasPrefix($0) }) ? ("designer-reference-\(theme)", "Approved Designer frame (wide reference)")
        : name.hasPrefix("local-first-pass") ? ("setup-reference-first-pass", "Approved frame 1e, light (wide reference)")
        : name.hasPrefix("local-setup-installing") || name.hasPrefix("local-setup-meet") ? ("setup-reference-meet", "Approved frame 1c, light (wide reference)")
        : ["before-connect", "connected"].contains(name) || name.hasPrefix("local-setup-ai") || name.hasPrefix("local-setup-queued") || name.hasPrefix("local-setup-connected") || name.hasPrefix("local-key-anthropic-empty") ? ("setup-reference-ai", "Approved frame 1d, light (wide reference, cropped)")
        : name.hasPrefix("local-reset-mismatch") || name.hasPrefix("local-reset-place") ? ("ai-connection-failed-reference-\(theme)", "Approved AI connection failure frame (wide reference)")
        : name.hasPrefix("local-reset-") ? ("ai-reset-failed-reference-\(theme)", "Approved AI reset frame (wide reference)")
        : nil
    if let frame {
        guard let actual = NSImage(contentsOfFile: file),
            let reference = NSImage(contentsOfFile: "../docs/images/\(frame.file).png") else {
            fatalError("Missing approved comparison image")
        }
        let width = actual.size.width
        let referenceHeight = reference.size.height * width / reference.size.width
        let height = max(actual.size.height, referenceHeight) + 42
        let comparison = NSImage(size: NSSize(width: width * 2 + 48, height: height + 16))
        comparison.lockFocus()
        NSColor.windowBackgroundColor.setFill()
        NSRect(origin: .zero, size: comparison.size).fill()
        actual.draw(in: NSRect(x: 16, y: height - 42 - actual.size.height, width: width, height: actual.size.height))
        reference.draw(in: NSRect(x: width + 32, y: height - 42 - referenceHeight, width: width, height: referenceHeight))
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.labelColor]
        ("Native Bridge, synthetic test data" as NSString).draw(at: NSPoint(x: 16, y: height - 26), withAttributes: attributes)
        (frame.label as NSString).draw(at: NSPoint(x: width + 32, y: height - 26), withAttributes: attributes)
        comparison.unlockFocus()
        guard let tiff = comparison.tiffRepresentation,
            let bitmap = NSBitmapImageRep(data: tiff),
            let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("Cannot compose approved comparison") }
        try png.write(to: URL(fileURLWithPath: "\(output)/\(name)-comparison.png"))
    }
    print("PASS \(name): real native frontend, synthetic IPC only")
    return rows
}
func enabledButton(_ label: String) -> AXUIElement? {
    let buttons = nativeElements().filter { element in
        ["AXButton", "AXCheckBox"].contains(attribute(element, "AXRole") as? String ?? "")
            && attribute(element, "AXEnabled") as? Bool == true
            && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == label })
    }
    return buttons.count == 1 ? buttons[0] : nil
}
func click(_ label: String, _ rows: [VNRecognizedTextObservation]) {
    guard let button = enabledButton(label) else { require(false, "Native button is disabled or missing: \(label)"); return }
    // A heading and a step can have the same words as the action. Click the
    // exact accessible button, never an OCR row that could name either heading.
    guard let bounds = nativeBounds(button), window()!.1.contains(bounds) else {
        require(false, "Native button is not visible: \(label)"); return
    }
    let p = CGPoint(x: bounds.midX, y: bounds.midY)
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
    }
    Thread.sleep(forTimeInterval: 1)
}
// Exercise the four-step list with only synthetic IPC, at the actual minimum and wide sizes.
waitForButton("Connect the AI")
for (phase, expected) in [("installing", "Installing, 1 GiB of 2 GiB"), ("starting", "45 seconds elapsed")] {
    try phase.write(to: setupProgress, atomically: true, encoding: .utf8)
    Thread.sleep(forTimeInterval: 0.6)
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        localSize(width, height)
        scroll(4000)
        _ = try capture("local-setup-\(phase)-\(size)", ["Getting your workspace ready", "Where agents run", "Get your workspace ready", expected])
        if phase == "installing" {
            // Show the file-derived tool/test rows, not just the card heading at the fold.
            scroll(-4000)
            require(nativeChatTextIsVisible("web_fetch"), "Meet tool name must be visible")
            _ = try capture("local-setup-meet-\(size)", ["Meet your first agent", "web-reader", "Read a web page"])
        }
    }
}
localSize(1280, 800)
scroll(4000)
click("Connect the AI", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("Connects when", -200)
    reveal("Cancel queued connection")
    _ = try capture("local-setup-queued-\(size)", ["Queued", "Cancel"])
}
reveal("Cancel queued connection")
click("Cancel queued connection", [])
localSize(1280, 800); scroll(4000)
click("Show details", [])
_ = try capture("local-setup-starting-details", ["45 seconds elapsed", "Starting the local cluster"])
try FileManager.default.removeItem(at: setupProgress)
Thread.sleep(forTimeInterval: 0.6)
click("Clusters", [])
waitForButton("Connect the AI")
// A running VM is not completed setup: its local row offers the current AI action.
var rows = try capture("local-places-wide", ["Choose where agents run", "This computer", "Connect the AI"])
require(enabledButton("Connect the AI") != nil && enabledButton("Manage") == nil && enabledButton("Set up") == nil,
        "VM-ready local card must offer Connect the AI, not Manage")
click("Connect the AI", rows)
rows = try capture("before-connect", ["Connect the AI"])
localSize(540, 640)
scroll(4000)
_ = try capture("local-setup-ai-minimum-top", ["Where agents run", "Get your workspace ready"])
_ = try seek("Connect the AI", -200)
_ = try capture("local-setup-ai-minimum", ["GitHub Copilot", "Connect the AI"])
// Earlier-installation record (frames 1g/1h): synthetic recovery IDs, no store, file or VM.
try "model-mismatch".write(to: setupProgress, atomically: true, encoding: .utf8)
press("Connect the AI")
waitForButton("Reset AI connection")
require(nativeTextExists("The AI couldn't connect") && nativeTextExists("Your workspace is running. Nothing is lost."), "Mismatch must name the AI failure and the safe workspace")
require(!["Remove sign-in", "Check again", "Connect the AI"].contains(where: anyButton), "Mismatch offers only reset recovery")
try resetCapture("mismatch", ["earlier setup", "Use another provider", "Copy details for help"], anchor: "Reset AI connection")
press("Clusters")
waitForButton("Reset AI connection")
try resetCapture("place", ["This computer", "Workspace running", "earlier setup"], anchor: "Reset AI connection")
press("Connect the AI")
waitForButton("Reset AI connection")
press("Reset AI connection")
try resetCapture("confirmation", ["Reset the AI connection?", "starts with this same reset", "Cancel"], anchor: "Reset")
press("Cancel")
waitForButton("Reset AI connection")
press("Use another provider")
require(nativeTextExists("Use another provider starts with this same reset."), "Use another provider must confirm the same reset")
press("Cancel")
try "reset-fail-credential".write(to: setupProgress, atomically: true, encoding: .utf8)
press("Reset AI connection")
press("Reset")
waitForButton("Reset again")
require(nativeTextExists("The AI connection couldn't be reset") && nativeTextExists("Bridge couldn't reset the AI connection"), "Reset failure needs its own headings")
require(!anyButton("Use another provider") && !anyButton("Remove sign-in"), "Reset failure offers only Reset again and Copy details")
try resetCapture("failed", ["couldn't reset the AI connection", "Try once more", "Copy details for help"], anchor: "Reset again")
press("Copy details for help")
waitForNativeText(["Details copied.", "Couldn't copy."])
try resetCapture("copy-notice", [nativeTextExists("Details copied.") ? "No keys or sign-in tokens are included." : "Couldn't copy."], anchor: "Copy details for help")
// The reset Copy button precedes its disclosure in keyboard order. Re-focus it
// after size captures, then activate the next control instead of guessing between two disclosures.
press("Copy details for help")
CGEvent(keyboardEventSource: nil, virtualKey: 48, keyDown: true)?.post(tap: .cghidEventTap)
CGEvent(keyboardEventSource: nil, virtualKey: 48, keyDown: false)?.post(tap: .cghidEventTap)
Thread.sleep(forTimeInterval: 0.1)
CGEvent(keyboardEventSource: nil, virtualKey: 49, keyDown: true)?.post(tap: .cghidEventTap)
CGEvent(keyboardEventSource: nil, virtualKey: 49, keyDown: false)?.post(tap: .cghidEventTap)
Thread.sleep(forTimeInterval: 0.3)
_ = try capture("local-reset-details-opened", [])
waitForNativeText(["Native credential removal not confirmed; saved record kept."])
try resetCapture("details", ["Native credential removal not confirmed; saved record kept.", "20000000-0000-4000-8000-000000000002"], anchor: "30000000-0000-4000-8000-000000000003")
try "reset-fail-record".write(to: setupProgress, atomically: true, encoding: .utf8)
press("Reset again")
waitForNativeText(["Reset failed again"])
waitForButton("Reset again")
require(nativeTextExists("Native credential removed; saved record removal not confirmed."), "Repeated failure must show its distinct custody stage")
try resetCapture("failed-again", ["Reset failed again", "Copy details for help"], anchor: "Reset again")
try "reset-pending".write(to: setupProgress, atomically: true, encoding: .utf8)
press("Reset again")
waitForNativeText(["Removing Bridge's saved AI connection"])
require(nativeTextExists("Resetting the AI connection"), "Resetting must name the operation")
require(!["Remove sign-in", "Connect the AI", "Reset again", "Reset AI connection"].contains(where: anyButton), "Resetting is a status, not an action")
try resetCapture("resetting", ["Resetting"], anchor: "Removing Bridge's saved AI connection")
try "reset-done".write(to: setupProgress, atomically: true, encoding: .utf8)
waitForNativeText(["Connection reset. Choose the AI again."])
waitForButton("Connect the AI")
require(!anyButton("Remove sign-in"), "Reset leaves no saved sign-in to remove")
try resetCapture("done", ["Connection reset. Choose the AI again.", "GitHub Copilot"], anchor: "Connect the AI")
try FileManager.default.removeItem(at: setupProgress)
localSize(1280, 800)
scroll(4000)
require(enabledButton("Remove sign-in") == nil, "Remove must be absent before the first sign-in attempt")
click("Connect the AI", rows)
rows = try capture("local-completed", ["This computer", "Local cluster ready", "Stop the local cluster", "Run your first test"])
require(!rows.contains { ($0.topCandidates(1).first?.string ?? "").range(of: "[Ss]tep [0-9]+ of [45]", options: .regularExpression) != nil },
        "Completed local page must not keep a setup counter")
click("Remove sign-in", rows)
rows = try capture("local-remove-confirmation", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Cancel", rows)
rows = try capture("connected", ["Connected", "Remove sign-in"])
require(nativeChatTextIsVisible("synthetic-user") && nativeChatTextIsVisible("gpt-5.6-terra"), "Connected Copilot must show the verified account and fixed model")
localSize(540, 640); scroll(4000)
_ = try seek("Saved GitHub sign-in", -200)
_ = try capture("local-setup-connected-minimum", ["GitHub Copilot", "gpt-5.6-terra"])
localSize(1280, 800); scroll(4000)
click("Remove sign-in", rows)
rows = try capture("local-remove-confirmed", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Remove sign-in", rows)
rows = try capture("local-remove-failed", ["Couldn't remove the saved sign-in", "Retry removal", "Copy details for help"])
require(nativeTextExists("The saved sign-in couldn't be removed") && !anyButton("Remove sign-in"), "Removal failure needs its own heading and no Remove")
require(enabledButton("Connect the AI") == nil, "Failed cleanup must offer removal retry, not connection")
click("Retry removal", rows)
rows = try capture("removed", ["no longer has a saved sign-in", "GitHub CLI stays signed in", "Connect the AI"])
click("Connect the AI", rows)
rows = try capture("reconnected", ["Connected", "Remove sign-in"])
// Workspace Stop is exercised after the original baseline coverage below.
// Reuse the setup-progress selector and the existing Places entry to remount real
// preflight for each bounded synthetic result. No native admission command runs.
// This synthetic reset is a fresh choice, so acknowledge Welcome explicitly;
// Stop no longer provides that unrelated presentation side effect.
try "preflight-ready".write(to: setupProgress, atomically: true, encoding: .utf8)
waitForButton("Get started")
click("Get started", [])
for state in ["ready", "unknown", "blocked", "memory", "mixed", "cluster"] {
    try "preflight-\(state)".write(to: setupProgress, atomically: true, encoding: .utf8)
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        localSize(1280, 800)
        scroll(4000)
        click("Clusters", [])
        click("Set up", [])
        // Completion is a status, not the presence of a retry control.
        var checksSettled = false
        for _ in 0..<100 {
            let strings = nativeElements().flatMap { element in
                ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
            }
            if strings.contains(where: { $0.contains("These checks do not change") || $0.contains("Fix the computer requirements") || $0.contains("Bridge couldn't check this computer yet.") }) { checksSettled = true; break }
            Thread.sleep(forTimeInterval: 0.1)
        }
        require(checksSettled, "Computer requirements did not settle")
        let choices = nativeElements().filter { attribute($0, "AXRole") as? String == "AXRadioButton" }
        require(choices.count == 2, "Preflight needs exactly two setup choices")
        let local = choices.first { element in
            ["AXTitle", "AXDescription"].contains { (attribute(element, $0) as? String)?.hasPrefix("On this computer") == true }
        }
        require(local.flatMap { attribute($0, "AXValue") as? NSNumber }?.intValue == 1, "Local setup must be selected by default")
        let next = nativeElements().filter { element in
            attribute(element, "AXRole") as? String == "AXButton"
                && ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == "Continue" }
        }
        require(next.count == 1, "Preflight needs one Continue")
        require((attribute(next[0], "AXEnabled") as? Bool == true) == !["blocked", "memory", "mixed", "cluster"].contains(state),
                "Only known local blockers may disable Continue")
        let strings = nativeElements().flatMap { element in
            ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
        }
        require(strings.contains { $0.contains("4.3 GB") } && strings.contains { $0.contains("Step 1 of 4") },
                "Local choice lost native-derived decimal GB or its step counter")
        require(enabledButton("Back to clusters") == nil && enabledButton("Choose where agents run") == nil, "Local setup must not have a header Back link")
        if ["ready", "unknown"].contains(state) {
            require(!strings.contains { $0.hasPrefix("Virtualization:") || $0.hasPrefix("Installed memory:") || $0 == "Not checked" },
                    "Only checks that ran and failed get rows")
            require(enabledButton("Check again") == nil && enabledButton("Copy details for help") == nil, "Nonfailed checks need no recovery controls")
        }
        if ["unknown", "mixed"].contains(state) {
            require(strings.contains { $0.contains("Bridge couldn't check this computer yet. It checks again when the workspace starts.") },
                    "Unknown checks need one quiet startup explanation")
        }
        if ["memory", "mixed"].contains(state) {
            require(strings.contains { $0.contains("Not enough installed memory") }, "Insufficient RAM must show its failed row")
        }
        if state == "cluster" {
            click("Use a cluster instead", [])
            require(nativeElements().contains { element in
                attribute(element, "AXFocused") as? Bool == true
                    && ["AXTitle", "AXDescription"].contains { (attribute(element, $0) as? String)?.hasPrefix("On a cluster you already have") == true }
            }, "Cluster fallback must focus its radio choice")
            waitForButton("Continue")
            require(!nativeElements().contains { element in
                ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == "Computer requirements" }
            }, "Cluster choice must unmount local checks")
        }
        localSize(width, height)
        scroll(4000)
        _ = try capture("local-preflight-\(state)-\(size)", ["On this computer"])
        scroll(-4000)
        _ = try capture("local-preflight-\(state)-\(size)-end", ["Continue"])
        let actionBounds = (state == "cluster" ? enabledButton("Continue") : next.first).flatMap(nativeBounds)
        require(actionBounds.map { window()!.1.contains($0) && $0.maxY <= window()!.1.maxY - 32 } == true,
                "Continue must remain reachable above the fixed footer at \(size)")
    }
}
// Check the selected cluster route without selecting local or using a cluster.
click("Continue", [])
_ = try capture("local-preflight-cluster-chooser", ["Add a cluster"])
require(enabledButton("Connect the AI") == nil, "Cluster route must not select local setup")
click("Back to clusters", [])
try FileManager.default.removeItem(at: setupProgress)
localSize(1280, 800)
scroll(4000)
click("Set up", [])
waitForButton("Continue")
scroll(-4000)
click("Continue", [])
rows = try capture("before-first-test", ["Connect the AI"])
click("Connect the AI", rows)
_ = try seek("Tests on the local cluster", -200)
let firstResult = try capture("local-first-test-result", ["Tests on the local cluster", "1 of 2 passed"])
require(!firstResult.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ").contains("Your agent selection could not be saved"), "Fixture rejected its owned agent selection")
// The native fixture reports the same finite Busy error as a held VM connection.
// This proves first-open renderer loading/recovery, not live guest discovery.
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try capture("local-catalog-first-loading-\(size)", ["Loading available tools and models"])
    require(!nativeTextExists("Tools could not be read from this cluster. Check Clusters or edit File view."), "Busy catalog must not become an error")
    require(enabledButton("Refresh tools and models") == nil, "Busy catalog must not require Refresh")
}
try "ready".write(to: catalogRelease, atomically: true, encoding: .utf8)
localSize(1280, 800); scroll(4000)
waitForButton("Add a tool"); click("Add a tool", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height))
    _ = try capture("local-catalog-first-open-\(size)", ["Find a tool", "Read a file"])
}
click("Close picker", [])
localSize(1280, 800); scroll(4000)
click("Clusters", [])
rows = try capture("local-first-test-failed", ["This computer", "Its test failed", "See what it answered"])
func reveal(_ label: String) {
    for _ in 0..<8 {
        let frame = window()!.1
        // The fixed 32px footer is inside the window but covers page controls.
        if let bounds = enabledButton(label).flatMap(nativeBounds), frame.contains(bounds), bounds.maxY <= frame.maxY - 32 { return }
        scroll(-200)
    }
    require(false, "Native control not visible: \(label)")
}
func waitForNativeText(_ text: String) {
    for _ in 0..<40 {
        if nativeElements().contains(where: { (attribute($0, "AXValue") as? String)?.contains(text) == true }) { return }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(false, "Native text did not appear: \(text)")
}
// Fresh-profile automatic outcomes use the explicit preflight fixture reset, never Stop.
// This restores the sample and reloads the renderer without its first-test claim; no VM or files.
for outcome in ["passed", "unknown"] {
    try outcome.write(to: firstTestOutcome, atomically: true, encoding: .utf8)
    localSize(1280, 800); scroll(4000)
    try "preflight-ready".write(to: setupProgress, atomically: true, encoding: .utf8)
    waitForButton("Set up")
    try FileManager.default.removeItem(at: setupProgress)
    click("Set up", [])
    waitForButton("Continue"); scroll(-4000); click("Continue", [])
    waitForButton("Connect the AI"); click("Connect the AI", [])
    if outcome == "passed" {
        waitForButton("Open web-reader")
        for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
            localSize(CGFloat(width), CGFloat(height)); scroll(4000)
            _ = try capture("local-first-pass-\(size)", ["Your first agent passed its test", "What did the page say?"])
            reveal("Open web-reader")
            _ = try capture("local-first-pass-\(size)-action", ["Open web-reader"])
        }
        require(enabledButton("Stop the local cluster") == nil, "The first-pass story must not offer Stop")
        localSize(1280, 800); scroll(4000); reveal("Open web-reader")
        click("Open web-reader", [])
        click("Clusters", [])
        require(enabledButton("Open web-reader") == nil, "The first-pass story replayed")
    } else {
        waitForNativeText("Result unknown")
        scroll(4000)
        for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
            localSize(CGFloat(width), CGFloat(height)); scroll(4000)
            _ = try seek("Result unknown", -200)
            _ = try capture("local-first-test-unknown-\(size)", ["Result unknown"])
        }
        require(enabledButton("Open web-reader") == nil && !nativeChatTextIsVisible("Your first agent passed its test"), "An unknown result must not celebrate")
    }
}
try FileManager.default.removeItem(at: firstTestOutcome)
localSize(1280, 800); scroll(4000)
func scroll(_ pixels: Int32) {
    let rect = window()!.1
    let event = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: pixels, wheel2: 0, wheel3: 0)
    event?.location = CGPoint(x: rect.maxX - 20, y: rect.midY)
    event?.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.3)
}
func seek(_ label: String, _ pixels: Int32) throws -> [VNRecognizedTextObservation] {
    for _ in 0..<6 {
        let seen = try capture("local-probe", [])
        // The large progress label must be visible in the pixels: AX can also report
        // the hidden setup panel's copy at coordinates inside this same window.
        if seen.contains(where: { ($0.topCandidates(1).first?.string ?? "").contains(label) }) || label != "Waiting for answers" && nativeChatTextIsVisible(label) {
            if label == "Run tests" && enabledButton(label) == nil { Thread.sleep(forTimeInterval: 0.3); continue }
            if ["Connect the AI", "Check again", "Change", "Remove key"].contains(label),
                enabledButton(label).flatMap(nativeBounds).map({ window()!.1.contains($0) }) != true {
                scroll(pixels); continue
            }
            return seen
        }
        scroll(pixels)
    }
    fatalError("Native local result control missing: \(label)")
}
func localSize(_ width: CGFloat, _ height: CGFloat) {
    let app = AXUIElementCreateApplication(child.processIdentifier)
    var windows: CFTypeRef?
    require(AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &windows) == .success, "Cannot read fixture window")
    let native = (windows as! [AXUIElement])[0]
    var origin = CGPoint(x: 0, y: 40)
    require(AXUIElementSetAttributeValue(native, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &origin)!) == .success, "Cannot position fixture window")
    var size = NSWindow.frameRect(forContentRect: CGRect(x: 0, y: 0, width: width, height: height), styleMask: [.titled, .closable, .miniaturizable, .resizable]).size
    require(AXUIElementSetAttributeValue(native, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!) == .success, "Cannot resize fixture window")
    Thread.sleep(forTimeInterval: 0.3)
    require(abs(window()!.1.width - size.width) < 3 && abs(window()!.1.height - size.height) < 3, "Fixture content size not applied")
}
// Key entry stays in the real password control; these actions never use a store or VM.
func chooseConnection(_ label: String) {
    let choices = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXRadioButton"
            && ["AXTitle", "AXDescription"].contains { (attribute(element, $0) as? String)?.hasPrefix(label) == true }
    }
    require(choices.count == 1, "Expected one radio choice: \(label)")
    require(AXUIElementPerformAction(choices[0], kAXPressAction as CFString) == .success, "Cannot select radio choice: \(label)")
}
func keyCapture(_ state: String, _ expected: [String], compact: Bool = false) throws {
    for size in compact ? ["wide", "compact"] : ["wide"] {
        if compact { localSize(size == "wide" ? 1280 : 540, size == "wide" ? 800 : 900) }
        scroll(4000)
        _ = try seek(expected.last!, -200)
        let seen = try capture("local-key-\(state)-\(size)", expected)
        require(!seen.contains { ($0.topCandidates(1).first?.string ?? "").contains("synthetic-only") }, "Password must be masked")
    }
    if compact { localSize(1280, 800); scroll(4000) }
}
// Change confirms removal of the saved sign-in before another choice.
click("Change", [])
click("Remove sign-in", [])
waitForButton("Connect the AI")
chooseConnection("Your own API key")
waitForButton("Anthropic")
try keyCapture("anthropic-empty", ["Your own API key", "OpenAI", "Anthropic", "API key", "claude-sonnet-5-5"], compact: true)
localSize(540, 640); scroll(4000)
_ = try seek("API key", -200)
_ = try capture("local-key-anthropic-empty-minimum", ["API key"])
_ = try seek("claude-sonnet-5-5", -200)
_ = try capture("local-key-anthropic-empty-minimum-model", ["claude-sonnet-5-5"])
localSize(1280, 800); scroll(4000)
click("OpenAI", [])
try keyCapture("openai-empty", ["OpenAI", "Anthropic", "API key", "gpt-4.1"])
click("Anthropic", [])
fillMessage("sk-ant-synthetic-only-not-a-real-key", secure: true)
try keyCapture("filled", ["API key", "claude-sonnet-5-5"], compact: true)
_ = try seek("Connect the AI", -200)
click("Connect the AI", [])
try keyCapture("checking", ["Checking", "Asking Anthropic"])
require(!anyButton("Remove key") && !anyButton("Remove sign-in"), "Remove must be absent while Connecting")
try Data().write(to: keyCheckRelease, options: .atomic)
waitForButton("Check again")
for (state, message) in [("rejected", "Anthropic didn't accept this key"),
                         ("unreachable", "workspace couldn't reach Anthropic"),
                         ("model-unavailable", "isn't listed for this key")] {
    try keyCapture(state, [message])
    fillMessage("sk-ant-synthetic-only-not-a-real-key", secure: true)
    _ = try seek("Check again", -200)
    click("Check again", [])
}
waitForButton("Change")
try keyCapture("connected", ["Anthropic with your API key", "claude-sonnet-5-5", "Connected", "Change"], compact: true)
// The existing first-test report now carries a finite credit result, not raw company text.
scroll(4000)
_ = try seek("Read its instructions", -200)
click("Read its instructions", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 900, "compact")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("Anthropic refused", -200)
    _ = try capture("local-key-credit-limited-\(size)", ["Anthropic refused the request", "out of credit or over its rate limit", "Check your Anthropic plan."])
}
localSize(1280, 800)
click("Clusters", [])
scroll(4000)
_ = try seek("Change", -200)
click("Change", [])
click("Remove key", [])
try keyCapture("change-failed", ["Key removal has not been confirmed", "Retry removal"])
require(nativeTextExists("Couldn't remove the saved key") && !anyButton("Remove key"), "Key removal failure needs its own heading and no Remove")
require(enabledButton("Anthropic") == nil, "Change must not permit replacement before cleanup")
click("Retry removal", [])
try keyCapture("changed", ["API key", "No AI connection."], compact: true)
fillMessage("sk-ant-synthetic-only-not-a-real-key", secure: true)
_ = try seek("Connect the AI", -200)
click("Connect the AI", [])
waitForButton("Remove key")
_ = try seek("Remove key", -200)
click("Remove key", [])
try keyCapture("remove-confirmation", ["saved key?", "Cancel", "Remove key"], compact: true)
click("Remove key", [])
try keyCapture("removed", ["API key", "No AI connection."])
chooseConnection("GitHub Copilot")
_ = try seek("Connect the AI", -200)
click("Connect the AI", [])
// Restore Copilot before the original result, editor and Chat captures.
scroll(4000)
rows = try seek("Read its instructions", -200)
click("Read its instructions", rows)
rows = try seek("Tests on the local cluster", -250)
_ = try capture("local-result-wide", ["Tests on the local cluster", "1 of 2 passed"])
_ = try seek("The answer did not include", -150)
_ = try capture("local-answer-wide", ["Which date is shown", "No date was given", "2099-12-31"])
localSize(540, 900)
scroll(4000)
_ = try seek("Tests on the local cluster", -250)
_ = try capture("local-result", ["Tests on the local cluster", "1 of 2 passed"])
_ = try seek("The answer did not include", -150)
_ = try capture("local-answer", ["Which date is shown", "No date was given", "2099-12-31"])
localSize(540, 640)
scroll(4000)
_ = try seek("Says which date", -150)
// Put the result above the fixed footer, not merely inside the window bounds.
scroll(-200)
_ = try capture("local-result-minimum", ["Says which date", "Asked"])
_ = try seek("The answer did not include", -100)
scroll(-180)
_ = try capture("local-story-minimum", ["No date was given", "2099-12-31"])
localSize(540, 900)
// Both local results and the cluster Send stay visible. This separate fixture
// has no send_start handler; opening/dismissing the warning cannot mutate anything.
localSize(1280, 800)
click("Clusters", [])
scroll(4000)
click("Clusters", [])
_ = try seek("Connect", -150)
click("Connect", [])
_ = try seek("Use this cluster", -150)
click("Use this cluster", [])
_ = try seek("Open your agent", -150)
click("Open your agent", [])
// A remote place blocks Chat: no Ready claim and a disabled New conversation, at both sizes.
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    waitForButton("Chat"); click("Chat", [])
    waitForChatText("Chat on local cluster")
    require(!nativeChatTextIsVisible("Ready for your message."), "Blocked Chat must not say Ready")
    require(anyButton("New conversation") && enabledButton("New conversation") == nil, "Blocked Chat must disable New conversation")
    _ = try capture("local-shell-chat-blocked-\(size)", ["Chat on local cluster"])
    _ = try seek("Chat is unavailable", -150)
    scroll(-100) // Keep the status clear of the fixed footer.
    _ = try capture("local-shell-chat-blocked-status-\(size)", ["Chat is unavailable."])
    scroll(4000)
    click("Agent", [])
}
func focusedButtonTitle() -> String? {
    var focused: CFTypeRef?
    let app = AXUIElementCreateApplication(child.processIdentifier)
    guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
        let focused else { return nil }
    let element = focused as! AXUIElement
    return attribute(element, "AXTitle") as? String ?? attribute(element, "AXDescription") as? String
}
for (width, height, size) in [(1280, 800, "wide"), (540, 900, "compact")] {
    localSize(CGFloat(width), CGFloat(height))
    scroll(4000)
    _ = try seek("Send and run tests", -200)
    guard let send = enabledButton("Send and run tests"), let before = nativeBounds(send) else {
        fatalError("Ready Send action missing beside local results")
    }
    click("Send and run tests", [])
    let warning = try capture("local-send-warning-\(size)", ["Local test", "failed", "Fix it first", "Send anyway"])
    guard let message = warning.first(where: { ($0.topCandidates(1).first?.string ?? "").contains("Local test") }),
        let fix = warning.first(where: { ($0.topCandidates(1).first?.string ?? "").contains("Fix it first") }) else {
        fatalError("Warning message and primary action need visible native bounds")
    }
    require((message.boundingBox.minY - fix.boundingBox.maxY) * window()!.1.height < 160,
            "Warning must fit its content rather than stretch to viewport height")
    require(focusedButtonTitle() == "Fix it first", "Warning must initially focus Fix it first")
    guard let after = nativeBounds(send) else { fatalError("Send action lost its bounds") }
    require(abs(before.minY - after.minY) < 2 && abs(before.minX - after.minX) < 2,
            "Warning must overlay, not shift the Send control")
    for down in [true, false] {
        CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: down)?.post(tap: .cghidEventTap)
    }
    Thread.sleep(forTimeInterval: 0.3)
    require(focusedButtonTitle() == "Send and run tests", "Escape must return focus to Send")
    click("Send and run tests", [])
    click("Fix it first", [])
    _ = try seek("Edit instructions", 150)
    _ = try capture("local-send-fix-result-\(size)", ["Which date is shown", "No date was given", "Edit instructions"])
}
// Return through the local row, without stopping either destination. The
// remaining pre-existing editor/Chat walk continues on This computer.
click("Clusters", [])
scroll(4000)
click("Clusters", [])
let localAction = nativeElements().first { element in
    guard attribute(element, "AXRole") as? String == "AXButton" else { return false }
    var parent = element
    for _ in 0..<12 {
        if attribute(parent, "AXDescription") as? String == "This computer" { return true }
        guard let next = attribute(parent, "AXParent") else { break }
        parent = next as! AXUIElement
    }
    return false
}
guard let localAction, let localBounds = nativeBounds(localAction), window()!.1.contains(localBounds) else {
    fatalError("This computer's action was not visible after warning proof")
}
for type in [CGEventType.leftMouseDown, .leftMouseUp] {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: CGPoint(x: localBounds.midX, y: localBounds.midY), mouseButton: .left)?.post(tap: .cghidEventTap)
}
Thread.sleep(forTimeInterval: 0.5)
_ = try seek("Use this computer", -150)
click("Use this computer", [])
click("Designer", [])
scroll(4000)
// Edit the fixture's Instructions field through the native text control. Its
// writer is in-memory only, so no creator file or credential can be changed.
scroll(4000)
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}
func nativeElements() -> [AXUIElement] {
    var elements = [AXUIElementCreateApplication(child.processIdentifier)]
    var index = 0
    while index < elements.count && elements.count < 4000 {
        let element = elements[index]; index += 1
        for key in ["AXChildren", "AXWindows"] {
            for item in attribute(element, key) as? [AXUIElement] ?? []
                where !elements.contains(where: { CFEqual($0, item) }) { elements.append(item) }
        }
    }
    return elements
}
func nativeBounds(_ element: AXUIElement) -> CGRect? {
    guard let position = attribute(element, "AXPosition"), CFGetTypeID(position) == AXValueGetTypeID(),
        let size = attribute(element, "AXSize"), CFGetTypeID(size) == AXValueGetTypeID() else { return nil }
    var origin = CGPoint.zero; var extent = CGSize.zero
    guard AXValueGetValue(position as! AXValue, .cgPoint, &origin)
        && AXValueGetValue(size as! AXValue, .cgSize, &extent)
        && extent.width > 0 && extent.height > 0 else { return nil }
    return CGRect(origin: origin, size: extent)
}
// Vision can misread small Inter failure values and elapsed labels. Require
// the exact expected native text, visibly inside this window, not OCR aliases.
func nativeResultTextIsVisible(_ expected: String) -> Bool {
    nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXStaticText",
            let text = attribute(element, "AXValue") as? String,
            (text == expected || (expected == "Observed for" ? text.hasPrefix("Observed for ") : text.hasPrefix("The answer did not include") && text.contains(expected))),
            let bounds = nativeBounds(element) else { return false }
        return bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)
    }
}
// Vision read the visible Stop workspace button with a lowercase initial.
// Exact, in-window accessibility controls prove the same property independently.
func nativeWorkspaceButtonIsVisible(_ expected: String) -> Bool {
    nativeElements().contains { element in
        guard ["AXButton", "AXCheckBox"].contains(attribute(element, "AXRole") as? String ?? ""),
            ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == expected }),
            let bounds = nativeBounds(element) else { return false }
        return window()!.1.contains(bounds)
    }
}
func nativeChatTextIsVisible(_ expected: String) -> Bool {
    nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXStaticText",
            let text = attribute(element, "AXValue") as? String,
            text.contains(expected), let bounds = nativeBounds(element) else { return false }
        // WebKit exposes clipped or screen-reader-only text with zero/one-pixel bounds.
        return bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)
    }
}
func waitForButton(_ label: String) {
    for _ in 0..<40 {
        if enabledButton(label) != nil { return }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(false, "Native Chat action did not become available: \(label)")
}
// Presence checks for state assertions; unlike enabledButton these include disabled controls.
func anyButton(_ label: String) -> Bool {
    nativeElements().contains { element in
        ["AXButton", "AXCheckBox"].contains(attribute(element, "AXRole") as? String ?? "")
            && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == label })
    }
}
func nativeTextExists(_ expected: String) -> Bool {
    nativeElements().contains { element in
        attribute(element, "AXRole") as? String == "AXStaticText" && (attribute(element, "AXValue") as? String)?.contains(expected) == true
    }
}
func waitForNativeText(_ any: [String]) {
    for _ in 0..<40 {
        if any.contains(where: nativeTextExists) { return }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(false, "Native text did not appear: \(any)")
}
func reachable(_ bounds: CGRect?) -> Bool {
    bounds.map { window()!.1.contains($0) && $0.maxY <= window()!.1.maxY - 32 } == true
}
// Scroll from the top until the exact control sits above the fixed footer, then click it.
func press(_ label: String) {
    scroll(4000)
    for _ in 0..<10 {
        if reachable(enabledButton(label).flatMap(nativeBounds)) { break }
        scroll(-200)
    }
    click(label, [])
}
// Each reset state at the wide size and the exact 540x640 minimum, beside its approved frame.
func resetCapture(_ state: String, _ expected: [String], anchor: String) throws {
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        localSize(width, height)
        scroll(4000)
        for _ in 0..<10 {
            if let button = enabledButton(anchor) { if reachable(nativeBounds(button)) { break } }
            else if nativeChatTextIsVisible(anchor) { break }
            scroll(-200)
        }
        _ = try capture("local-reset-\(state)-\(size)", expected + [anchor])
    }
    localSize(1280, 800)
    scroll(4000)
}
func waitForChatText(_ text: String) {
    for _ in 0..<40 {
        if nativeChatTextIsVisible(text) { return }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(false, "Native Chat text did not appear: \(text)")
}
func fillMessage(_ text: String, secure: Bool = false) {
    let label = secure ? "API key" : "Message"
    let fields = nativeElements().filter { element in
        (secure ? ["AXTextField", "AXSecureTextField"] : ["AXTextArea"]).contains(attribute(element, "AXRole") as? String ?? "")
            && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == label })
    }
    require(fields.count == 1, "Expected one labelled \(label) editor")
    let field = fields[0]
    if secure { require(attribute(field, "AXSubrole") as? String == "AXSecureTextField", "API key must use a secure field") }
    require(AXUIElementSetAttributeValue(field, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "Cannot focus \(label)")
    require(attribute(field, "AXFocused") as? Bool == true, "\(label) did not receive focus")
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
        event.flags = .maskCommand; event.post(tap: .cghidEventTap)
    }
    // Deliver text in short Unicode payloads, not an unpaced pair of events
    // per character that can flood the native event queue on a busy runner.
    let characters = Array(text.utf16)
    for start in stride(from: 0, to: characters.count, by: 16) {
        let units = Array(characters[start..<min(start + 16, characters.count)])
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
            units.withUnsafeBufferPointer { event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress!) }
            event.flags = []; event.post(tap: .cghidEventTap)
        }
    }
    Thread.sleep(forTimeInterval: 0.2)
    // Secure values may be hidden by AX; never read or log their contents.
    if !secure {
        let actual = attribute(field, "AXValue") as? String
        if actual != text { _ = try? capture("chat-input-failed", []) }
        require(actual == text, "Native keystrokes did not fill Chat Message: expected \(text.debugDescription), got \(String(describing: actual))")
    }
}
// Controls use the compiled frontend with a synthetic catalog, not real tools.
for (width, height, size) in [(1280, 800, "wide"), (900, 800, "medium"), (540, 900, "compact")] {
    localSize(CGFloat(width), CGFloat(height))
    scroll(4000)
    _ = try seek("Instructions and settings", -150)
    _ = try capture("local-controls-agent-\(size)", ["Instructions and settings"])
    _ = try seek("Add a tool", -150)
    waitForButton("Add a tool")
    _ = try capture("local-controls-fields-\(size)", ["Add a tool", "It thinks with"])
    click("Add a tool", [])
    let search = nativeElements().filter { element in
        ["AXTextField", "AXSearchField"].contains(attribute(element, "AXRole") as? String ?? "")
            && ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == "Find a tool" }
    }
    require(search.count == 1, "Expected the labeled picker search field")
    require(nativeBounds(search[0]).map { abs($0.height - 32) < 2 } == true, "Picker search must be 32px high")
    _ = try capture("local-controls-picker-\(size)", ["Find a tool", "Read a file"])
    let approvalReason = "Approval tools need autonomous coordination, which Bridge does not support for local tests or Chat."
    let approvalButtons = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXButton"
            && ["AXTitle", "AXDescription"].contains { (attribute(element, $0) as? String)?.contains("request_approval") == true }
    }
    require(approvalButtons.count == 1 && attribute(approvalButtons[0], "AXEnabled") as? Bool == false, "Approval tool must stay disabled locally")
    require(nativeResultTextIsVisible(approvalReason), "Approval reason must be fully visible in the picker")
    _ = try capture("local-approval-tool-\(size)", ["Find a tool", "Ask for approval", approvalReason])
    guard let close = enabledButton("Close picker"), let bounds = nativeBounds(close) else {
        fatalError("Labeled picker X missing")
    }
    require(abs(bounds.width - 26) < 2 && abs(bounds.height - 26) < 2, "Picker X must be 26px square")
    click("Close picker", [])
    require(enabledButton("Close picker") == nil, "X did not dismiss picker")
    require(attribute(enabledButton("Add a tool")!, "AXFocused") as? Bool == true, "X did not return focus")
    if size == "compact" {
        // Responsive placement must not trap focus in the editor/picker stack.
        for down in [true, false] { CGEvent(keyboardEventSource: nil, virtualKey: 48, keyDown: down)?.post(tap: .cghidEventTap) }
        Thread.sleep(forTimeInterval: 0.3)
        require(attribute(enabledButton("Add a tool")!, "AXFocused") as? Bool != true, "Tab stayed on Add a tool")
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: 48, keyDown: down)
            event?.flags = .maskShift
            event?.post(tap: .cghidEventTap)
        }
        Thread.sleep(forTimeInterval: 0.3)
        require(attribute(enabledButton("Add a tool")!, "AXFocused") as? Bool == true, "Shift-Tab did not return to Add a tool")
    }
    click("Add a tool", [])
    for down in [true, false] { CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: down)?.post(tap: .cghidEventTap) }
    Thread.sleep(forTimeInterval: 0.3)
    require(enabledButton("Close picker") == nil, "Escape did not dismiss picker")
    click("Add a tool", [])
    let outside = CGPoint(x: window()!.1.minX + 5, y: window()!.1.minY + 90)
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: outside, mouseButton: .left)?.post(tap: .cghidEventTap)
    }
    Thread.sleep(forTimeInterval: 0.3)
    require(enabledButton("Close picker") == nil, "Outside click did not dismiss picker")
    _ = try seek("More settings", -150)
    click("More settings", [])
    _ = try capture("local-disclosure-settings-\(size)", ["More settings", "Description"])
    click("More settings", [])
}
scroll(4000)
// Match the fixture's known Instructions value, not a label plus a guessed
// offset which can land outside the editor after responsive reflow.
let editors = nativeElements().filter {
    attribute($0, "AXRole") as? String == "AXTextArea"
        && (attribute($0, "AXValue") as? String)?.hasPrefix("Answer from the web page named in the request.") == true
}
require(editors.count == 1, "Expected the fixture Instructions editor")
let editor = editors[0]
require(AXUIElementSetAttributeValue(editor, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "Cannot focus fixture Instructions")
Thread.sleep(forTimeInterval: 0.3)
guard let editorBounds = nativeBounds(editor) else { fatalError("Native Instructions bounds missing") }
let visibleEditor = editorBounds.intersection(window()!.1)
require(!visibleEditor.isEmpty && !visibleEditor.isNull, "Native Instructions editor is offscreen")
let editorPoint = CGPoint(x: visibleEditor.midX, y: visibleEditor.midY)
for type in [CGEventType.leftMouseDown, .leftMouseUp] {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: editorPoint, mouseButton: .left)?.post(tap: .cghidEventTap)
}
Thread.sleep(forTimeInterval: 0.2)
require(attribute(editor, "AXFocused") as? Bool == true, "Native Instructions did not receive focus")
for down in [true, false] {
    let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
    event.flags = .maskCommand; event.post(tap: .cghidEventTap)
}
for character in "Changed fixture instructions.".utf16 {
    var unit = character
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
        event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
        event.flags = []; event.post(tap: .cghidEventTap)
    }
}
Thread.sleep(forTimeInterval: 1)
require((attribute(editor, "AXValue") as? String)?.contains("Changed fixture instructions.") == true, "Native keystrokes did not edit Instructions")
// Focusing Instructions scrolls below the compact test card; return to its start.
scroll(4000)
_ = try seek("Not run since your edit", -250)
_ = try capture("local-stale", ["Not run since your edit"])
// Shell evidence beside the approved Designer frame: Agent, File, then Send and Past Sends opened on request.
// Blocked Chat is captured in the remote section above. The first (wide) pass sees the unused Send hidden; opening it
// keeps it retained, so the minimum pass must see it already visible.
func exactTextIsVisible(_ expected: String) -> Bool {
    nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXStaticText", attribute(element, "AXValue") as? String == expected,
            let bounds = nativeBounds(element) else { return false }
        return bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)
    }
}
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    click("Agent", [])
    require(nativeElements().contains { attribute($0, "AXRole") as? String == "AXPopUpButton" }, "Header agent picker missing")
    require(anyButton("Send to a cluster") && anyButton("Past Sends"), "Header Send entry and Past Sends must always exist")
    if size == "wide" { require(!anyButton("Send and run tests"), "An unused Send must start hidden") }
    else { require(anyButton("Send and run tests"), "Send opened earlier must stay retained and visible") }
    _ = try capture("local-shell-agent-\(size)", ["Instructions and settings"])
    click("File", [])
    _ = try capture("local-shell-file-\(size)", ["web-reader.agent.md"])
    click("Agent", [])
    click("Send to a cluster", [])
    // The header action already reveals and scrolls the card into view.
    require(exactTextIsVisible("Send your agent") && nativeChatTextIsVisible("Team clusters are early in this preview."), "Send headline and early-preview text must be visible together")
    require(anyButton("Send and run tests") && enabledButton("Send and run tests") == nil, "Without a cluster, the opened Send must be visible but disabled")
    _ = try capture("local-shell-send-\(size)", ["Send your agent", "Team clusters are early in this preview."])
    press("Past Sends")
    _ = try seek("Saved results and limits", -150)
    scroll(-100)
    _ = try capture("local-shell-history-\(size)", ["Past Sends", "Saved results and limits"])
    press("Past Sends")
}
localSize(1280, 800)
// Chat runs before the final Run, which intentionally stays busy forever in
// this fixture. These actions change only the fixture's in-memory transcript.
localSize(1280, 800)
scroll(4000)
click("Chat", [])
_ = try capture("chat-empty-wide", ["Chat on local cluster", "Message"])
localSize(540, 900)
_ = try capture("chat-empty-compact", ["Chat on local cluster", "Message"])
fillMessage("Remember the checksum ALPHA.")
waitForButton("Send message")
click("Send message", [])
waitForChatText("The checksum is ALPHA.")
_ = try capture("chat-first-compact", ["You", "Agent", "The checksum is ALPHA.", "<script>fixture</script>"])
localSize(1280, 800)
_ = try capture("chat-first-wide", ["The checksum is ALPHA.", "<script>fixture</script>"])
fillMessage("What checksum did I ask you to remember?")
waitForButton("Send message")
click("Send message", [])
waitForChatText("You asked me to remember ALPHA.")
_ = try capture("chat-followup-wide", ["You asked me to remember ALPHA."])
localSize(540, 900)
_ = try capture("chat-followup-compact", ["You asked me to remember ALPHA."])
click("New conversation", [])
fillMessage("Read another page.")
waitForButton("Send message")
rows = try seek("Send message", -150)
click("Send message", rows)
waitForChatText("Read another page.")
_ = try capture("chat-new-version-compact", ["Conversation 2", "Read another page."])
localSize(1280, 800)
_ = try capture("chat-new-version-wide", ["Conversation 2", "Read another page."])
require(enabledButton("Stop") != nil, "Working Chat must offer Stop")
_ = try capture("chat-working-wide", ["Stop", "Read another page."])
localSize(540, 900)
_ = try capture("chat-working-compact", ["Stop", "Read another page."])
click("Stop", [])
waitForChatText("Execution termination is unconfirmed.")
waitForButton("New conversation")
_ = try capture("chat-stop-ready-compact", ["Ready", "Execution termination is unconfirmed."])
localSize(1280, 800)
_ = try capture("chat-stop-ready-wide", ["Ready", "Execution termination is unconfirmed."])
// Capture card actions before the final test deliberately holds the fixture busy.
click("Overview", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 900, "compact"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height))
    scroll(4000)
    _ = try capture("local-controls-overview-\(size)", ["Your agents", "Refresh", "web-reader", "Failed", "Run tests"])
}
click("By agent", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 900, "compact"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height))
    scroll(4000)
    _ = try capture("local-overview-by-agent-\(size)", ["Your agents", "web-reader", "This computer"])
}
click("By cluster", [])
click("Designer", [])
scroll(4000)
click("Agent", [])
localSize(540, 900)
scroll(4000)
rows = try seek("Run tests", -150)
click("Run tests", rows)
_ = try seek("Waiting for answers", -150)
_ = try capture("local-running", ["Waiting for answers", "Observed for"])
localSize(1280, 800)
scroll(4000)
_ = try seek("Waiting for answers", -250)
_ = try capture("local-running-wide", ["Waiting for answers", "Observed for", "Stop", "Copies the checksum from the page", "Awaiting result"])
localSize(540, 640)
scroll(4000)
_ = try seek("Stop", -100)
scroll(-400)
_ = try capture("local-running-minimum", ["Stop", "Waiting for answers", "Awaiting result"])
click("Stop", [])
_ = try seek("Cancelling", -100)
_ = try capture("local-stopping-minimum", ["Cancelling"])
localSize(540, 900)
scroll(4000)
click("Clusters", [])
_ = try capture("local-manage-compact", ["This computer", "Remove sign-in"])
// Keep the completed local and removal states reviewable at the compact size too.
_ = try seek("Remove sign-in", -200)
click("Remove sign-in", [])
_ = try capture("local-remove-confirmation-compact", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Cancel", [])
click("Clusters", [])
_ = try capture("local-places-compact", ["Choose where agents run", "This computer"])
// Stop/restart uses real UI actions. The existing selector settles only pending
// synthetic IPC; no VM, credentials or provider is accessed.
localSize(1280, 800)
scroll(4000)
click("Manage", [])
func workspacePhase(_ phase: String) throws {
    try phase.write(to: setupProgress, atomically: true, encoding: .utf8)
    Thread.sleep(forTimeInterval: 1.2)
}
func workspaceCapture(_ state: String, _ expected: [String]) throws {
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        localSize(width, height)
        scroll(4000)
        _ = try capture("local-workspace-\(state)-\(size)", expected)
    }
    localSize(1280, 800)
    scroll(4000)
}
try workspacePhase("workspace-wait")
click("Stop the local cluster", [])
try workspaceCapture("confirmation", ["Stop the workspace?", "Stop workspace", "Keep running"])
click("Keep running", [])
require(enabledButton("Stop the local cluster") != nil, "Keep running must not stop the workspace")
click("Stop the local cluster", [])
click("Stop workspace", [])
try workspaceCapture("stopping", ["This computer", "Workspace", "Stopping"])
require(enabledButton("Start") == nil, "Start must be disabled until stop acknowledgement")
try workspacePhase("workspace-stopped")
waitForButton("Start")
try workspaceCapture("stopped", ["This computer", "Workspace", "Stopped", "Start"])
_ = try capture("local-stopped", ["This computer", "Stopped", "Start"])
require(!nativeElements().contains { attribute($0, "AXRole") as? String == "AXRadioButton" }, "Stop must not return to first-run choices")
click("Start", [])
waitForButton("Cancel")
try workspaceCapture("starting", ["This computer", "Starting", "0:18 so far", "Cancel"])
try workspacePhase("workspace-wait")
click("Cancel", [])
try workspacePhase("workspace-stopped")
waitForButton("Start")
try workspaceCapture("cancelled", ["This computer", "Stopped", "Start"])
click("Start", [])
try workspacePhase("workspace-failed")
waitForButton("Try again")
try workspaceCapture("failed", ["This computer", "Couldn't start", "Try again"])
try workspacePhase("workspace-wait")
click("Try again", [])
try workspacePhase("workspace-running")
waitForButton("Stop")
try workspaceCapture("running", ["This computer", "Running", "Stop"])
try workspacePhase("workspace-wait")
click("Stop", [])
click("Stop workspace", [])
try workspacePhase("workspace-stopped")
waitForButton("Start")
try workspacePhase("workspace-unavailable")
try workspaceCapture("unavailable", ["This computer", "Status unavailable"])
require(enabledButton("Start") == nil, "Unreadable status must not authorize Start")
try workspacePhase("workspace-stopped")
waitForButton("Start")
click("Start", [])
try workspacePhase("workspace-running")
waitForButton("Stop")
try workspacePhase("workspace-wait")
click("Stop", [])
click("Stop workspace", [])
try workspacePhase("workspace-closed")
try workspaceCapture("closed", ["This computer", "Stopped", "Quit Bridge and reopen it"])
require(enabledButton("Start") == nil, "Terminal closed state must not offer an inert Start")
// Closed and failed are terminal: reset only the bounded fixture selector for
// independent Stop failures, rather than duplicating the full restart walk.
try workspacePhase("workspace-reset-ready")
waitForButton("Stop")
try workspacePhase("workspace-wait")
click("Stop", [])
click("Stop workspace", [])
try workspacePhase("workspace-stop-failed")
try workspaceCapture("stop-failed", ["This computer", "Couldn't stop", "Try again"])
require(enabledButton("Try again") == nil, "Unconfirmed Stop must not authorize Try again")
require(enabledButton("Start") == nil, "Unconfirmed Stop must not authorize Start")

try workspacePhase("workspace-reset-ready")
waitForButton("Stop")
try workspacePhase("workspace-wait")
click("Stop", [])
click("Stop workspace", [])
try workspacePhase("workspace-stop-lost-reply")
try workspaceCapture("stop-lost-reply", ["This computer", "Status unavailable", "Workspace status is unavailable"])
require(enabledButton("Stop") == nil && enabledButton("Start") == nil, "Lost Stop reply must pause workspace actions")
try workspacePhase("workspace-stopped")
waitForButton("Start")
try workspaceCapture("stop-reply-recovered", ["This computer", "Stopped", "Start"])
