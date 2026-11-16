// CI-only integrated debug shell. No workspace selection, VM, credentials or injected Ready state.
import AppKit
import ApplicationServices
import Vision
import ImageIO

let args = CommandLine.arguments
var app: NSRunningApplication?
var currentStage = "preflight"
var diagnosticRoot: AXUIElement?
defer {
    if let app, !app.isTerminated {
        failureEvidence()
        fputs("Walk failed before native quit; forcing only the CI-owned process down.\n", stderr)
        app.forceTerminate()
    }
}
func check(_ value: Bool, _ message: String) {
    if !value {
        fputs("FAIL [STAGE: \(currentStage)]: \(message)\n", stderr)
        failureEvidence()
        // Only this walk's process, after an assertion failure; never counted as a clean stop.
        app?.forceTerminate()
        exit(1)
    }
}
check(args.count == 4, "Usage: native-lifecycle-macos.swift OUTPUT DEBUG_APP light|dark")
let (output, bundle, theme) = (args[1], args[2], args[3])
let env = ProcessInfo.processInfo.environment
check(env["GITHUB_ACTIONS"] == "true" && ["light", "dark"].contains(theme), "Hosted CI walk only")
let home = URL(fileURLWithPath: env["HOME"]!)
// CI builds this flavor separately so observing setup does not consume release welcome state.
let identifier = "ai.orka.bridge.lifecycle"
let data = home.appendingPathComponent("Library/Application Support/\(identifier)")
let selected = data.appendingPathComponent("setup-v1/local-vm").path
check(!FileManager.default.fileExists(atPath: selected), "Existing saved local selection; will not launch or erase it")
check(!FileManager.default.fileExists(atPath: data.appendingPathComponent("local-vm-v1").path), "Existing VM data; refusing walk")
if theme == "light" { check(!FileManager.default.fileExists(atPath: data.path), "Expected fresh CI application profile") }
let source = env["BRIDGE_DEV_IMAGE_DIR"] ?? ""
check(!source.isEmpty, "Expected empty source directory")
check(try FileManager.default.contentsOfDirectory(atPath: source).isEmpty, "Expected empty source directory")
check(AXIsProcessTrusted(), "CI runner needs Accessibility permission for real native UI actions")
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
func wait(_ message: String, _ predicate: () -> Bool) {
    for _ in 0..<100 {
        if predicate() { return }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    }
    check(false, message)
}
@discardableResult
func command(_ executable: String, _ arguments: [String]) throws -> Process {
    let p = Process(); p.executableURL = URL(fileURLWithPath: executable); p.arguments = arguments
    try p.run()
    wait("Command timed out: \(executable)") { !p.isRunning }
    check(p.terminationStatus == 0, "Command failed: \(executable)")
    return p
}
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var result: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success ? result : nil
}
func nodes(_ root: AXUIElement) -> [AXUIElement] {
    var all = [root]; var i = 0
    // Bounded AX traversal, including native status-item menus rather than renderer hooks.
    while i < all.count && all.count < 4000 {
        let element = all[i]; i += 1
        for key in ["AXChildren", "AXWindows"] {
            for child in attribute(element, key) as? [AXUIElement] ?? [] where !all.contains(where: { CFEqual($0, child) }) { all.append(child) }
        }
        for key in ["AXMenuBar", "AXExtrasMenuBar", "AXShownMenuUIElement"] {
            if let value = attribute(element, key), CFGetTypeID(value) == AXUIElementGetTypeID() {
                let child = value as! AXUIElement
                if !all.contains(where: { CFEqual($0, child) }) { all.append(child) }
            }
        }
    }
    return all
}
func text(_ element: AXUIElement) -> String {
    ["AXTitle", "AXDescription", "AXHelp"].compactMap { attribute(element, $0) as? String }.joined(separator: " | ")
}
func actions(_ element: AXUIElement) -> [String] {
    var names: CFArray?
    return AXUIElementCopyActionNames(element, &names) == .success ? names as? [String] ?? [] : []
}
func pressable(_ element: AXUIElement, _ label: String) -> Bool {
    // WebKit can expose aria-pressed buttons as AXCheckBox/AXToggle (Core-AAM).
    ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == label }
        && attribute(element, "AXEnabled") as? Bool == true && actions(element).contains("AXPress")
}
// A Places row action can carry its row name after the visible verb; match the verb as a leading word.
func pressableAction(_ element: AXUIElement, _ label: String) -> Bool {
    attribute(element, "AXRole") as? String == "AXButton"
        && ["AXTitle", "AXDescription"].contains { key in
            guard let value = attribute(element, key) as? String else { return false }
            return value == label || value.hasPrefix(label + " ")
        }
        && attribute(element, "AXEnabled") as? Bool == true && actions(element).contains("AXPress")
}
func shownStrings(_ root: AXUIElement) -> [String] {
    nodes(root).flatMap { element in ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String } }
}
func writableTextArea(_ element: AXUIElement) -> Bool {
    var writable = DarwinBoolean(false)
    return attribute(element, "AXRole") as? String == "AXTextArea" && AXUIElementIsAttributeSettable(element, "AXValue" as CFString, &writable) == .success && writable.boolValue
}
func failureEvidence() {
    guard let app, !app.isTerminated else { return }
    let report = "STAGE: \(currentStage)\n" + nodes(diagnosticRoot ?? AXUIElementCreateApplication(app.processIdentifier)).prefix(100).map { element in
        "role=\(attribute(element, "AXRole") as? String ?? "?") text=\(String(text(element).prefix(200)).debugDescription) enabled=\(String(describing: attribute(element, "AXEnabled") as? Bool)) actions=\(String(describing: actions(element).prefix(20)).prefix(200))"
    }.joined(separator: "\n") + "\n"
    fputs(report, stderr)
    do { try report.write(toFile: "\(output)/failure-ax.txt", atomically: true, encoding: .utf8) }
    catch { fputs("Cannot save AX diagnostics: \(error)\n", stderr) }
    // Capture the real display (including menus or a missing window) before cleanup.
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture"); p.arguments = ["-x", "\(output)/failure.png"]
    do {
        try p.run()
        for _ in 0..<100 {
            if !p.isRunning { break }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
        if p.isRunning { p.terminate(); fputs("Failure screenshot timed out\n", stderr) }
        else { fputs("Failure screencapture status: \(p.terminationStatus)\n", stderr) }
    } catch { fputs("Cannot capture failure screenshot: \(error)\n", stderr) }
}
func find(_ root: AXUIElement, _ stage: String, _ predicate: (AXUIElement) -> Bool) -> AXUIElement {
    currentStage = stage; diagnosticRoot = root; print("STAGE: \(stage)")
    var result: AXUIElement?
    wait("Native AX control missing") { result = nodes(root).first(where: predicate); return result != nil }
    return result!
}
func press(_ element: AXUIElement) {
    check(AXUIElementPerformAction(element, "AXPress" as CFString) == .success, "Cannot activate native AX control: \(text(element))")
}
func click(_ element: AXUIElement) {
    guard let position = attribute(element, "AXPosition"), CFGetTypeID(position) == AXValueGetTypeID(),
        let size = attribute(element, "AXSize"), CFGetTypeID(size) == AXValueGetTypeID() else {
        check(false, "Native click target has no accessible bounds"); return
    }
    var point = CGPoint.zero; var extent = CGSize.zero
    check(AXValueGetValue(position as! AXValue, .cgPoint, &point) && AXValueGetValue(size as! AXValue, .cgSize, &extent) && extent.width > 0 && extent.height > 0, "Invalid native click bounds")
    point.x += extent.width / 2; point.y += extent.height / 2
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
    }
}
func windowInfo() -> (CGWindowID, CGRect)? {
    let list = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
    guard let w = list.first(where: { ($0[kCGWindowOwnerPID as String] as? Int32) == app?.processIdentifier && ($0[kCGWindowLayer as String] as? Int) == 0 }),
        let id = w[kCGWindowNumber as String] as? UInt32,
        let bounds = w[kCGWindowBounds as String] as? [String: Any],
        let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return nil }
    return (id, rect)
}
func capture(_ name: String, menu: Bool = false) throws {
    if !menu, let rect = windowInfo()?.1 {
        // File view now grows with its content. Restore the page heading after
        // end-of-file typing before testing its visible title in the capture.
        let scroll = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1,
            wheel1: 1000, wheel2: 0, wheel3: 0)
        // A posted mouse move is asynchronous; bind the wheel to the page gutter
        // instead of letting it inherit the previous position inside the textarea.
        scroll?.location = CGPoint(x: rect.maxX - 20, y: rect.midY)
        scroll?.post(tap: .cghidEventTap)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    }
    let file = "\(output)/\(name).png"
    let options = menu ? ["-x", file] : ["-x", "-o", "-l", String(windowInfo()!.0), file]
    try command("/usr/sbin/screencapture", options)
    let request = VNRecognizeTextRequest(); request.recognitionLevel = .accurate
    try VNImageRequestHandler(url: URL(fileURLWithPath: file)).perform([request])
    let seen = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
    if menu {
        check(seen.contains("Open Bridge") && seen.contains("Stop the local cluster and quit"), "Native menu not drawn in screenshot: \(seen)")
    } else {
        check(seen.contains("web-reader.agent.md"), "Reopened window lost File editor view: \(seen)")
        // Render the PNG's profile into sRGB bytes; NSColor calibration changed the sampled values.
        guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: file) as CFURL, nil),
            let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
            let pixel = image.cropping(to: CGRect(x: CGFloat(image.width - 8), y: CGFloat(image.height - 8), width: 1, height: 1)),
            let space = CGColorSpace(name: CGColorSpace.sRGB) else { fatalError("Cannot decode screenshot") }
        var rgba = [UInt8](repeating: 0, count: 4)
        let sampled = rgba.withUnsafeMutableBytes { bytes in
            guard let context = CGContext(data: bytes.baseAddress, width: 1, height: 1,
                bitsPerComponent: 8, bytesPerRow: 4, space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
                else { return false }
            context.draw(pixel, in: CGRect(x: 0, y: 0, width: 1, height: 1))
            return true
        }
        check(sampled, "Cannot sample screenshot in sRGB")
        let expected: [CGFloat] = theme == "dark" ? [26, 29, 33] : [228, 226, 219]
        let actual = rgba.prefix(3).map { CGFloat($0) }
        check(zip(actual, expected).allSatisfy { abs($0 - $1) < 8 }, "Window did not follow requested Estuary system theme: \(actual)")
    }
}
func launch() throws -> AXUIElement {
    check(!FileManager.default.fileExists(atPath: selected) && !FileManager.default.fileExists(atPath: data.appendingPathComponent("local-vm-v1").path), "Saved selection or VM data appeared; refusing to resume")
    check(NSRunningApplication.runningApplications(withBundleIdentifier: identifier).isEmpty, "Bridge is already running")
    // open supplies the override explicitly; release smoke uses its unchanged environment.
    try command("/usr/bin/open", [bundle, "--env", "BRIDGE_DEV_IMAGE_DIR=\(source)"])
    wait("Bridge did not launch") {
        app = NSRunningApplication.runningApplications(withBundleIdentifier: identifier).first
        return app != nil
    }
    app!.activate(options: [])
    wait("Main window did not appear") { windowInfo() != nil }
    return AXUIElementCreateApplication(app!.processIdentifier)
}
func menu(_ root: AXUIElement) -> [AXUIElement] {
    guard let extras = attribute(root, "AXExtrasMenuBar") else { fatalError("Missing native status menu bar") }
    let item = find(extras as! AXUIElement, "status-item") { attribute($0, "AXRole") as? String == "AXMenuBarItem" && text($0).contains("Bridge") }
    // AXPress reports success without opening tray-icon's mouse-driven menu.
    // Click the real item at its native bounds, as in the existing OCR walk.
    click(item)
    let open = find(root, "menu-open") { attribute($0, "AXTitle") as? String == "Open Bridge" }
    let stop = find(root, "menu-stop") { attribute($0, "AXTitle") as? String == "Stop the local cluster and quit" }
    check(attribute(open, "AXEnabled") as? Bool == true && attribute(stop, "AXEnabled") as? Bool == true, "Native menu commands disabled")
    return [open, stop]
}
func key(_ code: CGKeyCode, flags: CGEventFlags = []) {
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)!
        event.flags = flags; event.post(tap: .cghidEventTap)
    }
}
let root = try launch()
let pid = app!.processIdentifier
let originalID = windowInfo()!.0
var main = find(root, "main-window") { attribute($0, "AXRole") as? String == "AXWindow" }
press(find(main, "Clusters") { pressable($0, "Clusters") })
// The dark walk reuses the light walk's profile, so the welcome may already be dismissed.
wait("Welcome or Places did not appear") { nodes(main).contains { pressable($0, "Get started") || pressableAction($0, "Set up") } }
if let start = nodes(main).first(where: { pressable($0, "Get started") }) { press(start) }
// Places owns the local entry: this computer's Set up action beside an empty cluster list.
let setUp = find(main, "places-set-up") {
    pressableAction($0, "Set up") && shownStrings(main).contains { $0.contains("No other clusters added yet") }
}
try command("/usr/sbin/screencapture", ["-x", "-o", "-l", String(windowInfo()!.0), "\(output)/places-empty.png"])
press(setUp)
let requirements = find(main, "computer-requirements-panel") { text($0).contains("Computer requirements") }
_ = find(requirements, "computer-requirements") { pressable($0, "Check again") }
// Required installed memory stays visible; image and maximum disk facts live in What's inside.
press(find(main, "workspace-inside") { pressable($0, "What's inside") })
currentStage = "local-size-block"; print("STAGE: \(currentStage)")
wait("Local size facts did not settle") {
    let shown = shownStrings(main)
    return [#"about \d+\.\d GB of installed memory"#, #"image size is \d+\.\d GB after installation, not the amount downloaded"#, #"data disk can grow to \d+\.\d GB"#].allSatisfy { pattern in
        shown.contains { $0.range(of: pattern, options: .regularExpression) != nil }
    } && !shown.contains { $0.hasPrefix("Reading") || $0.contains("Unavailable in this build") }
}
// Capture the runner observations as drawn, without selecting local or asserting Ready.
try command("/usr/sbin/screencapture", ["-x", "-o", "-l", String(windowInfo()!.0), "\(output)/computer-requirements.png"])
check(!FileManager.default.fileExists(atPath: selected), "Preflight selected a workspace")
press(find(main, "Designer") { pressable($0, "Designer") })
// A writable Agent-view textarea proves the owned example loaded, not a
// temporary read-only preview that will remount and reset its view toggle.
_ = find(main, "editable-owned-example") { writableTextArea($0) }
press(find(main, "File") { pressable($0, "File") })
let editor = find(main, "writable-editor") { writableTextArea($0) }
check(AXUIElementSetAttributeValue(editor, "AXFocused" as CFString, kCFBooleanTrue) == .success, "Cannot focus real editor")
key(125, flags: .maskCommand)
let marker = " lifecycle-\(UUID().uuidString)"
for character in marker.utf16 {
    var unit = character
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
        event.flags = []; event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit); event.post(tap: .cghidEventTap)
    }
}
let owned = home.appendingPathComponent("Orka/agents/web-reader/web-reader.agent.md")
wait("Native editor change not saved") { (try? String(contentsOf: owned, encoding: .utf8))?.contains(marker) == true }
func retained() {
    check(app!.processIdentifier == pid && !app!.isTerminated, "Original process lost")
    check(windowInfo()?.0 == originalID, "Main native window replaced")
    // AX proxies can be regenerated on hide/show; they are not native-window
    // identity. The original CGWindowID above plus the non-persisted File view
    // and edited text below test the real window/webview retention contract.
    main = find(root, "retained-window") { attribute($0, "AXRole") as? String == "AXWindow" }
    check((attribute(find(main, "retained-editor") { attribute($0, "AXRole") as? String == "AXTextArea" }, "AXValue") as? String)?.contains(marker) == true, "File view/draft lost on reopen")
    check(NSWorkspace.shared.frontmostApplication?.processIdentifier == pid, "Reopen did not focus original process")
    check(!FileManager.default.fileExists(atPath: selected), "Walk accidentally selected a workspace")
}
func closeMain() {
    guard let close = attribute(main, "AXCloseButton") else { fatalError("Missing native Close button") }
    press(close as! AXUIElement)
}
closeMain()
wait("Native Close did not hide") { windowInfo() == nil }
check(!app!.isTerminated, "Close quit instead of hiding")
let commands = menu(root)
try capture("menu-hidden", menu: true)
press(commands[0])
wait("Open Bridge did not reopen/focus") { windowInfo() != nil && NSWorkspace.shared.frontmostApplication?.processIdentifier == pid }
retained(); try capture("reopened-menu")
closeMain()
wait("Close before Dock activation did not hide") { windowInfo() == nil }
try command("/usr/bin/open", [bundle]) // LaunchServices delivers Dock-style Reopen.
wait("Dock activation did not reopen/focus") { windowInfo() != nil && NSWorkspace.shared.frontmostApplication?.processIdentifier == pid }
retained(); try capture("reopened-dock")
closeMain()
wait("Close before second launch did not hide") { windowInfo() == nil }
try command("\(bundle)/Contents/MacOS/orka-bridge", [])
wait("Second launch did not show/focus hidden window") { windowInfo() != nil && NSWorkspace.shared.frontmostApplication?.processIdentifier == pid }
retained(); try capture("reopened-second-launch-hidden")
check(AXUIElementSetAttributeValue(main, "AXMinimized" as CFString, kCFBooleanTrue) == .success, "Cannot minimize main window")
wait("Window did not minimize") { attribute(main, "AXMinimized") as? Bool == true }
try command("\(bundle)/Contents/MacOS/orka-bridge", [])
wait("Second executable launch did not unminimize/focus") { attribute(main, "AXMinimized") as? Bool == false && NSWorkspace.shared.frontmostApplication?.processIdentifier == pid }
retained(); try capture("reopened-second-launch-minimized")
check(NSRunningApplication.runningApplications(withBundleIdentifier: identifier).map(\.processIdentifier) == [pid], "Second Bridge instance survived")
let stop = menu(root)[1]
try capture("menu-stop", menu: true)
press(stop)
wait("Stop the local cluster and quit did not exit") { app!.isTerminated }
// A separate launch proves standard AppKit Quit, not a synthetic terminate selector.
let quitRoot = try launch()
guard let appMenu = attribute(quitRoot, "AXMenuBar") else { fatalError("Missing native application menu bar") }
press(find(appMenu as! AXUIElement, "application-menu") { attribute($0, "AXRole") as? String == "AXMenuBarItem" && attribute($0, "AXTitle") as? String == "Bridge" })
press(find(quitRoot, "menu-quit") { attribute($0, "AXTitle") as? String == "Quit Bridge" })
wait("Standard Quit did not exit") { app!.isTerminated }
check(!FileManager.default.fileExists(atPath: selected) && !FileManager.default.fileExists(atPath: data.appendingPathComponent("local-vm-v1").path), "Saved local selection or VM data appeared")
let evidence = "PASS \(theme): native Close/hide, tray menu/Open, same PID \(pid)/window \(originalID), File view and edited text retained, Dock activation, hidden/minimized second launches, menu stop, standard Quit. Integrated debug shell only; empty image source, no VM/Ready/provider/cluster/session logout proof.\n"
try evidence.write(toFile: "\(output)/evidence.txt", atomically: true, encoding: .utf8)
print(evidence)
