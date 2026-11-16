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
try "installing".write(to: setupProgress, atomically: true, encoding: .utf8)
defer { try? FileManager.default.removeItem(at: setupProgress) }
child.environment = ProcessInfo.processInfo.environment.merging(["BRIDGE_MODEL_CHECK_RELEASE": keyCheckRelease.path, "BRIDGE_SETUP_PROGRESS": setupProgress.path]) { _, fixture in fixture }
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
            .replacingOccurrences(of: "APl key", with: "API key").contains(value)
        let nativeText = !found && name.hasPrefix("local-") && (nativeResultTextIsVisible(value) || name.hasPrefix("local-key-") && nativeChatTextIsVisible(value))
        let chatText = !found && name.hasPrefix("chat-") && nativeChatTextIsVisible(value)
        require(found || nativeText || chatText, "Missing \(value) in \(name): \(text)")
    }
    guard let bitmap = NSBitmapImageRep(data: try Data(contentsOf: URL(fileURLWithPath: file))),
          let colour = bitmap.colorAt(x: bitmap.pixelsWide - 20, y: bitmap.pixelsHigh - 20)?.usingColorSpace(.deviceRGB)
        else { fatalError("Cannot sample native theme") }
    let brightness = (colour.redComponent + colour.greenComponent + colour.blueComponent) / 3
    require(theme == "dark" ? brightness < 0.25 : brightness > 0.8, "Wrong system theme")
    if ["local-result", "local-answer", "local-story", "local-running", "local-stopping"].contains(where: { name.hasPrefix($0) }) {
        guard let actual = NSImage(contentsOfFile: file),
            let reference = NSImage(contentsOfFile: "../docs/images/designer-reference-\(theme).png") else {
            fatalError("Missing Designer comparison image")
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
        ("Approved Designer frame (wide reference)" as NSString).draw(at: NSPoint(x: width + 32, y: height - 26), withAttributes: attributes)
        comparison.unlockFocus()
        guard let tiff = comparison.tiffRepresentation,
            let bitmap = NSBitmapImageRep(data: tiff),
            let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("Cannot compose Designer comparison") }
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
    }
}
localSize(1280, 800)
scroll(4000)
click("Show details", [])
_ = try capture("local-setup-starting-details", ["45 seconds elapsed", "Starting the local cluster"])
try FileManager.default.removeItem(at: setupProgress)
Thread.sleep(forTimeInterval: 0.6)
click("Choose where agents run", [])
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
localSize(1280, 800)
scroll(4000)
require(enabledButton("Remove sign-in") == nil, "Remove must be absent before the first sign-in attempt")
click("Connect the AI", rows)
rows = try capture("local-completed", ["Local cluster", "Local cluster ready", "Stop the local cluster", "Run your first test"])
require(!rows.contains { ($0.topCandidates(1).first?.string ?? "").range(of: "[Ss]tep [0-9]+ of [45]", options: .regularExpression) != nil },
        "Completed local page must not keep a setup counter")
click("Remove sign-in", rows)
rows = try capture("local-remove-confirmation", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Cancel", rows)
rows = try capture("connected", ["Connected", "Remove sign-in"])
click("Remove sign-in", rows)
rows = try capture("local-remove-confirmed", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Remove sign-in", rows)
rows = try capture("remove-failed", ["Retry removal", "Copy details for help"])
require(enabledButton("Connect the AI") == nil, "Failed cleanup must offer removal retry, not connection")
click("Retry removal", rows)
rows = try capture("removed", ["no longer has a saved sign-in", "GitHub CLI stays signed in", "Connect the AI"])
click("Connect the AI", rows)
rows = try capture("reconnected", ["Connected", "Remove sign-in"])
// Only the fixture's finite in-memory state changes; no VM or files are touched.
click("Stop the local cluster", rows)
rows = try capture("local-stopped", ["Set up this computer", "This computer", "Continue", "Back to clusters"])
require(enabledButton("Stop the local cluster") == nil, "Stop must leave the local preflight page")
click("Continue", rows)
rows = try capture("before-first-test", ["Connect the AI"])
click("Connect the AI", rows)
_ = try seek("Tests on the local cluster", -200)
let firstResult = try capture("local-first-test-result", ["Tests on the local cluster", "1 of 2 passed"])
require(!firstResult.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ").contains("Your agent selection could not be saved"), "Fixture rejected its owned agent selection")
click("Clusters", [])
rows = try capture("local-first-test-failed", ["Local cluster", "Its test failed", "See what it answered"])
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
        if seen.contains(where: { ($0.topCandidates(1).first?.string ?? "").contains(label) }) || nativeChatTextIsVisible(label) {
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
    require(choices.count == 1, "Expected one AI connection choice")
    require(AXUIElementPerformAction(choices[0], kAXPressAction as CFString) == .success, "Cannot select AI connection")
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
click("Change", [])
chooseConnection("Your own API key")
waitForButton("Anthropic")
try keyCapture("anthropic-empty", ["Your own API key", "OpenAI", "Anthropic", "API key", "claude-sonnet-5-5"], compact: true)
click("OpenAI", [])
try keyCapture("openai-empty", ["OpenAI", "Anthropic", "API key", "gpt-4.1"])
click("Anthropic", [])
fillMessage("sk-ant-synthetic-only-not-a-real-key", secure: true)
try keyCapture("filled", ["API key", "claude-sonnet-5-5"], compact: true)
_ = try seek("Connect the AI", -200)
click("Connect the AI", [])
try keyCapture("checking", ["Checking", "Asking Anthropic"])
try Data().write(to: keyCheckRelease, options: .atomic)
waitForButton("Check again")
for (state, message) in [("rejected", "Anthropic didn't accept this key"),
                         ("unreachable", "local cluster couldn't reach Anthropic"),
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
try keyCapture("change-failed", ["Key removal has not been confirmed", "Retry removal"])
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
        return window()!.1.contains(bounds)
    }
}
func nativeChatTextIsVisible(_ expected: String) -> Bool {
    nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXStaticText",
            let text = attribute(element, "AXValue") as? String,
            text.contains(expected), let bounds = nativeBounds(element) else { return false }
        return window()!.1.contains(bounds)
    }
}
func waitForButton(_ label: String) {
    for _ in 0..<40 {
        if enabledButton(label) != nil { return }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(false, "Native Chat action did not become available: \(label)")
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
_ = try capture("local-manage-compact", ["Back to clusters", "Remove sign-in"])
// Keep the completed local and removal states reviewable at the compact size too.
_ = try seek("Remove sign-in", -200)
click("Remove sign-in", [])
_ = try capture("local-remove-confirmation-compact", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Cancel", [])
click("Back to clusters", [])
_ = try capture("local-places-compact", ["Choose where agents run", "This computer"])
