import AppKit
import CoreGraphics
import ApplicationServices
import Vision
import ImageIO

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
var restoreTemporaryTests: (() throws -> Void)?
var restoreTemporaryAgent: (() throws -> Void)?
func fail(_ message: String) -> Never {
    fputs("\(message)\n", stderr)
    do { try restoreTemporaryTests?() }
    catch { fputs("Cannot restore original test file: \(error)\n", stderr) }
    do { try restoreTemporaryAgent?() }
    catch { fputs("Cannot restore original agent file: \(error)\n", stderr) }
    restoreDisplay()
    exit(1)
}
func require(_ condition: Bool, _ message: String) {
    if !condition { fail(message) }
}
let output = CommandLine.arguments[1]
let theme = CommandLine.arguments[3]
require(["light", "dark"].contains(theme), "Expected light or dark theme")
require(NSRunningApplication.runningApplications(withBundleIdentifier: "ai.orka.bridge").isEmpty,
        "Quit running Bridge before running the native walk")
let workspaceData = FileManager.default.homeDirectoryForCurrentUser
    .appendingPathComponent("Library/Application Support/ai.orka.bridge")
func requireNoLocalWorkspace() {
    for path in ["setup-v1/local-vm", "local-vm-v1", "image-cache-v1"] {
        require(!FileManager.default.fileExists(atPath: workspaceData.appendingPathComponent(path).path),
                "Native smoke must not select, download or start a local workspace: \(path)")
    }
}
// Hosted Macs default to 1024x768, which constrains a 900pt content window.
// Only reconfigure the disposable CI display, never an operator's desktop.
if CGDisplayBounds(display).width < 1280 || CGDisplayBounds(display).height < 1080 {
    require(ProcessInfo.processInfo.environment["GITHUB_ACTIONS"] == "true",
            "The native walk needs a display of at least 1280x1080 logical points")
    let options = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
    let modes = CGDisplayCopyAllDisplayModes(display, options) as? [CGDisplayMode] ?? []
    print("CI display modes: \(modes.map { "\($0.width)x\($0.height)" })")
    guard let previous = CGDisplayCopyDisplayMode(display),
        let larger = modes.filter({ $0.isUsableForDesktopGUI() && $0.width >= 1280 && $0.height >= 1080 })
            .min(by: { $0.width * $0.height < $1.width * $1.height }) else {
        fail("No CI display mode can fit the required native window")
    }
    previousDisplayMode = previous
    require(CGDisplaySetDisplayMode(display, larger, nil) == .success, "Cannot enlarge CI display")
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 1))
    require(CGDisplayBounds(display).width >= 1280 && CGDisplayBounds(display).height >= 1080,
            "CI display mode did not take effect: \(CGDisplayBounds(display))")
}
defer { restoreDisplay() }
func launch() throws -> NSRunningApplication {
requireNoLocalWorkspace()
let opened = Process()
opened.executableURL = URL(fileURLWithPath: "/usr/bin/open")
var arguments = [CommandLine.arguments[2]]
if let kubeconfig = ProcessInfo.processInfo.environment["BRIDGE_WALK_KUBECONFIG"] {
    arguments += ["--env", "KUBECONFIG=\(kubeconfig)"]
}
opened.arguments = arguments
try opened.run()
opened.waitUntilExit()
require(opened.terminationStatus == 0, "Cannot open Bridge")
var running: NSRunningApplication?
for _ in 0..<30 {
    running = NSRunningApplication.runningApplications(withBundleIdentifier: "ai.orka.bridge").first
    if running != nil { break }
    Thread.sleep(forTimeInterval: 1)
}
guard let launched = running else { fail("Bridge process did not appear") }
return launched
}
var app = try launch()
defer {
    app.terminate()
    for _ in 0..<30 {
        if app.isTerminated { break }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    }
    require(app.isTerminated, "Bridge did not quit after the native walk")
}
app.activate(options: [])
func window() -> (CGWindowID, CGRect)? {
    let list = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
    guard let w = list.first(where: {
        ($0[kCGWindowOwnerPID as String] as? Int32) == app.processIdentifier &&
        ($0[kCGWindowLayer as String] as? Int) == 0
    }), let id = w[kCGWindowNumber as String] as? UInt32,
        let bounds = w[kCGWindowBounds as String] as? [String: Any],
        let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return nil }
    return (id, rect)
}
func waitForWorkspaceRenderer() {
var found = false
let rendererDeadline = Date(timeIntervalSinceNow: 30)
while Date() < rendererDeadline {
    if let (_, frame) = window() {
        let buttons = nativeElements().filter { element in
            attribute(element, "AXRole") as? String == "AXButton"
                && attribute(element, "AXEnabled") as? Bool == true
                && nativeBounds(element).map { frame.contains($0) } == true
        }
        let names = Set(buttons.flatMap { element in
            ["AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
        })
        // The first-job screen intentionally hides navigation. Returning setup
        // still proves the complete tab bar before this walk proceeds.
        if names.contains("See an example first") || Set(["Overview", "Designer", "Clusters"]).isSubset(of: names)
            && nativeTextIsVisible("Choose where agents run") { found = true; break }
    }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
}
require(found, "Bridge native window, rendered navigation and page controls did not appear within the startup budget")
}
waitForWorkspaceRenderer()
func mouse(_ type: CGEventType, _ point: CGPoint) {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
}
func clickText(_ label: String, in observations: [VNRecognizedTextObservation]) {
    // Vision can include the SVG chevron in a disclosure label.
    let pattern = #"^\W*"# + NSRegularExpression.escapedPattern(for: label) + #"$"#
    guard let target = observations.first(where: { $0.topCandidates(1).first?.string.range(of: pattern, options: .regularExpression) != nil }) else {
        // Preserve the label's actual pointer action when small dark text is
        // unreadable to Vision; require one exact, fully visible native label.
        let labels = nativeElements().filter {
            attribute($0, "AXRole") as? String == "AXStaticText"
                && attribute($0, "AXValue") as? String == label
        }.compactMap { nativeBounds($0) }.filter { window()!.1.contains($0) }
        require(!labels.isEmpty, "No visible native label: \(label)")
        // WebKit can expose both a label and its text node. They must refer to
        // the same on-screen hit region; distinct/ambiguous labels still fail.
        let hit = labels.dropFirst().reduce(labels[0]) { $0.intersection($1) }
        require(!hit.isNull && !hit.isEmpty, "Ambiguous native label \(label): \(labels)")
        print("Native label \(label): \(labels), hit \(hit)")
        let point = CGPoint(x: hit.midX, y: hit.midY)
        mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
        Thread.sleep(forTimeInterval: 0.5)
        return
    }
    let rect = window()!.1
    let point = CGPoint(x: rect.minX + target.boundingBox.midX * rect.width,
                        y: rect.minY + (1 - target.boundingBox.midY) * rect.height)
    mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
    Thread.sleep(forTimeInterval: 0.5)
}
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}
func nativeElements(_ root: AXUIElement? = nil) -> [AXUIElement] {
    var nodes = [root ?? AXUIElementCreateApplication(app.processIdentifier)]
    var index = 0
    while index < nodes.count && nodes.count < 4000 {
        let element = nodes[index]; index += 1
        for key in ["AXChildren", "AXWindows", "AXSheets"] {
            for child in attribute(element, key) as? [AXUIElement] ?? []
                where !nodes.contains(where: { CFEqual($0, child) }) { nodes.append(child) }
        }
    }
    return nodes
}
func nativeRegion(_ name: String) -> AXUIElement {
    let matches = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXGroup"
            && attribute(element, "AXSubrole") as? String == "AXLandmarkRegion"
            && ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == name }
    }
    require(matches.count == 1, "Expected one named native region \(name), found \(matches.count)")
    return matches[0]
}
func nativeBounds(_ element: AXUIElement) -> CGRect? {
    guard let position = attribute(element, "AXPosition"), CFGetTypeID(position) == AXValueGetTypeID(),
        let size = attribute(element, "AXSize"), CFGetTypeID(size) == AXValueGetTypeID() else { return nil }
    var point = CGPoint.zero; var extent = CGSize.zero
    guard AXValueGetValue(position as! AXValue, .cgPoint, &point),
        AXValueGetValue(size as! AXValue, .cgSize, &extent), extent.width > 0, extent.height > 0 else { return nil }
    return CGRect(origin: point, size: extent)
}
func clickDesignerView(_ name: String) {
    let matches = nativeElements().filter { element in
        ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == name }
            && attribute(element, "AXEnabled") as? Bool == true
            && (attribute(element, "AXRole") as? String == "AXCheckBox" || attribute(element, "AXRole") as? String == "AXButton")
    }.compactMap { nativeBounds($0) }.filter { window()!.1.contains($0) }
    require(matches.count == 1, "Cannot identify the visible Designer \(name) control")
    let point = CGPoint(x: matches[0].midX, y: matches[0].midY)
    mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
    Thread.sleep(forTimeInterval: 0.3)
}
// Places actions are native buttons; a row action may carry the row name after its verb.
func nativeButtons(_ label: String, root: AXUIElement? = nil) -> [AXUIElement] {
    nativeElements(root).filter { element in
        guard let role = attribute(element, "AXRole") as? String else { return false }
        // WebKit exposes the context's aria-haspopup trigger as a popup button.
        return (role == "AXButton" || label == "kubectl context" && role == "AXPopUpButton")
            && ["AXTitle", "AXDescription"].contains { key in
                guard let value = attribute(element, key) as? String else { return false }
                return value == label || value.hasPrefix(label + " ")
                    || ["published-checksum", "has-a-date"].contains(label) && value.contains(label)
            }
    }
}
// Page content only; the fixed header and footer can overlay scrolled controls. The compact tab
// bar is shorter than the wide one, so take its bottom from the actual native tab, not a fixed 110px.
func bodyBounds() -> CGRect {
    let rect = window()!.1
    let tabsBottom = nativeButtons("Clusters").compactMap({ nativeBounds($0) }).map({ $0.maxY }).max()
    let top = (tabsBottom ?? rect.minY) + 8
    // Job intake hides both tab and status bars. Do not reserve a phantom
    // footer that would exclude its visible privacy line from native evidence.
    let bottom = tabsBottom == nil ? rect.maxY - 8 : rect.maxY - 32
    return CGRect(x: rect.minX, y: top, width: rect.width, height: max(0, bottom - top))
}
func visibleButtons(_ label: String) -> [CGRect] {
    nativeButtons(label).filter { attribute($0, "AXEnabled") as? Bool == true }
        .compactMap { nativeBounds($0) }.filter { bodyBounds().contains($0) }
}
func setOverviewExpanded(_ title: String, _ expanded: Bool) {
    let label = "Show places for \(title)"
    let controls = nativeButtons(label)
    require(controls.count == 1, "Cannot identify Overview disclosure \(label)")
    if attribute(controls[0], "AXExpanded") as? Bool != expanded { clickButton(label) }
}
func clickButton(_ label: String) {
    for attempt in 0..<8 {
        let found = visibleButtons(label)
        require(found.count < 2, "Ambiguous native button \(label): \(found)")
        if let button = found.first {
            let point = CGPoint(x: button.midX, y: button.midY)
            mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
            Thread.sleep(forTimeInterval: 0.5)
            return
        }
        scrollMain(attempt == 0 ? 1000 : -200)
    }
    fail("No visible enabled native button: \(label)")
}
// Whole accessibility tree, below the fold included: for counting, not for proving visibility.
func nativeStrings() -> [String] {
    nativeElements().flatMap { element in
        ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
    }
}
func nativeTextBounds(_ expected: String, heading: Bool = false, visibleOnly: Bool = true, exact: Bool = false, root: AXUIElement? = nil) -> [CGRect] {
    guard window() != nil else { return [] }
    // Use the same native viewport as controls: compact tabs are shorter than wide tabs.
    // Below-the-fold bounds can guide scrolling, but never prove visibility.
    let body = bodyBounds()
    var matches: [CGRect] = []
    for element in nativeElements(root) {
        guard let role = attribute(element, "AXRole") as? String else { continue }
        if ["AXStaticText", "AXHeading", "AXDisclosureTriangle"].contains(role) && (!heading || role == "AXHeading") {
            var values = ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
            if heading {
                // The cluster name now lives in page h1 > code, not a result-card h2.
                // WebKit can expose that code text on a child rather than the heading itself.
                values += nativeElements(element).dropFirst().filter { attribute($0, "AXRole") as? String == "AXStaticText" }
                    .compactMap { attribute($0, "AXValue") as? String }
            }
            if values.contains(where: { (heading || exact) ? $0 == expected : $0.contains(expected) }),
                let bounds = nativeBounds(element), (!visibleOnly || body.contains(bounds)) { matches.append(bounds) }
        }
        guard !heading, role == "AXGroup",
            let children = attribute(element, "AXChildren") as? [AXUIElement] else { continue }
        // WebKit can flatten an inline badge into its surrounding group.
        // Join adjacent text siblings only on one visible, touching line.
        var text = ""
        var line: CGRect?
        func finishLine() {
            if let bounds = line, (exact ? text == expected : text.contains(expected)) { matches.append(bounds) }
            text = ""; line = nil
        }
        for child in children {
            guard attribute(child, "AXRole") as? String == "AXStaticText",
                let value = attribute(child, "AXValue") as? String,
                let bounds = nativeBounds(child), (!visibleOnly || body.contains(bounds)) else { finishLine(); continue }
            if let prior = line, abs(prior.midY - bounds.midY) > 2
                || bounds.minX < prior.maxX - 2 || bounds.minX > prior.maxX + 4 { finishLine() }
            text += value
            line = line.map { $0.union(bounds) } ?? bounds
        }
        finishLine()
    }
    return matches
}
func nativeTextIsVisible(_ expected: String, exact: Bool = false) -> Bool { !nativeTextBounds(expected, exact: exact).isEmpty }
@discardableResult
func clickTab(_ page: String, _ observations: [VNRecognizedTextObservation]) -> AXUIElement {
    // Small Inter labels can be grouped with subtitles by Vision. Identify the
    // real enabled native button, but still pointer-click it and verify the
    // rendered destination in the next capture (not AXPress or renderer hooks).
    let header = observations.filter { 1 - $0.boundingBox.maxY < 0.10 }
        .compactMap { $0.topCandidates(1).first?.string }
    print("NAV \(page): header OCR \(header)")
    let matches = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXButton"
            && attribute(element, "AXEnabled") as? Bool == true
            && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == page })
    }
    require(matches.count == 1, "Expected one enabled native tab named \(page), found \(matches.count)")
    let target = matches[0]
    guard let position = attribute(target, "AXPosition"), CFGetTypeID(position) == AXValueGetTypeID(),
        let size = attribute(target, "AXSize"), CFGetTypeID(size) == AXValueGetTypeID() else {
        fail("Native tab has no bounds: \(page)")
    }
    var origin = CGPoint.zero; var extent = CGSize.zero
    require(AXValueGetValue(position as! AXValue, .cgPoint, &origin)
        && AXValueGetValue(size as! AXValue, .cgSize, &extent), "Cannot read native tab bounds")
    let rect = window()!.1
    let bounds = CGRect(origin: origin, size: extent)
    require(extent.width > 0 && extent.height > 0 && rect.contains(bounds)
        && bounds.maxY <= rect.minY + 110, "Native tab is not visible in the header: \(page), \(bounds)")
    let point = CGPoint(x: bounds.midX, y: bounds.midY)
    mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
    Thread.sleep(forTimeInterval: 0.5)
    return target
}
func key(_ code: CGKeyCode, shift: Bool = false) {
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)!
        event.flags = shift ? .maskShift : []
        event.post(tap: .cghidEventTap)
    }
    Thread.sleep(forTimeInterval: 0.3)
}
@discardableResult
// Vision joins “1of 1” in the smaller result heading; normalize spacing, not the count or verdict.
func normalizedSendText(_ text: String) -> String {
    text.replacingOccurrences(of: #"(?<=\d)\s*of\s*(?=\d)"#, with: " of ", options: .regularExpression)
        .split(whereSeparator: \.isWhitespace).joined(separator: " ")
}
func sendFrameVisible(_ observations: [VNRecognizedTextObservation]) -> Bool {
    let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
    let markers = ["Send your agent", "Team cluster", "Ready to send", "Tests on your cluster", "passed", "evaluating tests", "Planned agent", "question", "answer", "Earlier files", "Not run since your edit", "Stopped watching",
        "What you send", "What happened", "Earlier result", "Send needs attention", "Running tests",
        "Cancelling tests", "Cancellation requested.", "Cannot cancel with this cluster identity", "This Orka cannot cancel tests.", "Cancellation not confirmed.", "Tests cancelled on the cluster.", "Tests finished before cancellation."]
    return markers.contains { text.localizedCaseInsensitiveContains($0) }
}
func capture(_ name: String, _ page: String, searching: Bool = false, afterFrame: (() throws -> Void)? = nil) throws -> [VNRecognizedTextObservation] {
    let file = "\(output)/\(name).png"
    let command = Process()
    command.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    // Exclude the window shadow so OCR coordinates are relative to the window.
    command.arguments = ["-x", "-o", "-l", String(window()!.0), file]
    try command.run(); command.waitUntilExit()
    require(command.terminationStatus == 0, "Cannot capture Bridge")
    try afterFrame?()
    if name.hasPrefix("new-agent-") && name != "new-agent-probe" || name.hasPrefix("cluster-") && name != "cluster-probe" || name.hasPrefix("places-") {
        let designer = name.hasPrefix("new-agent-")
        let referenceFile = designer ? "designer-reference-\(theme)" : "cluster-reference-light"
        guard let actual = NSImage(contentsOfFile: file), let reference = NSImage(contentsOfFile: "../docs/images/\(referenceFile).png") else { fail("Missing native comparison reference") }
        let width = actual.size.width, referenceHeight = reference.size.height * actual.size.width / reference.size.width
        let height = max(actual.size.height, referenceHeight) + 42
        let comparison = NSImage(size: NSSize(width: width * 2 + 48, height: height + 16))
        comparison.lockFocus()
        NSColor.windowBackgroundColor.setFill(); NSRect(origin: .zero, size: comparison.size).fill()
        actual.draw(in: NSRect(x: 16, y: height - 42 - actual.size.height, width: width, height: actual.size.height))
        reference.draw(in: NSRect(x: width + 32, y: height - 42 - referenceHeight, width: width, height: referenceHeight))
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.labelColor]
        ("Native Bridge, disposable fixture" as NSString).draw(at: NSPoint(x: 16, y: height - 26), withAttributes: attributes)
        ((designer ? "Designer design system (no dedicated New agent frame)" : "Approved cluster cards (wide reference)") as NSString).draw(at: NSPoint(x: width + 32, y: height - 26), withAttributes: attributes)
        comparison.unlockFocus()
        guard let tiff = comparison.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff), let png = bitmap.representation(using: .png, properties: [:]) else { fail("Cannot compose native comparison") }
        try png.write(to: URL(fileURLWithPath: "\(output)/\(name)-comparison.png"))
    }
    // Overview's scrolled evidence can paint an overlay scrollbar at the window edge.
    let chromeInset = name == "overview" || name.hasPrefix("overview-") ? 20 : 8
    // Render the PNG's profile into sRGB bytes; NSColor calibration changed the sampled values.
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: file) as CFURL, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
        let pixel = image.cropping(to: CGRect(x: CGFloat(image.width - chromeInset), y: CGFloat(image.height - 8), width: 1, height: 1)),
        let space = CGColorSpace(name: CGColorSpace.sRGB) else { fail("Cannot decode screenshot") }
    var rgba = [UInt8](repeating: 0, count: 4)
    let sampled = rgba.withUnsafeMutableBytes { bytes in
        guard let context = CGContext(data: bytes.baseAddress, width: 1, height: 1,
            bitsPerComponent: 8, bytesPerRow: 4, space: space,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
            else { return false }
        context.draw(pixel, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        return true
    }
    require(sampled, "Cannot sample screenshot in sRGB")
    var expected: [CGFloat] = theme == "dark" ? [9, 9, 10] : [255, 255, 255]
    if name.hasPrefix("cluster-picker-open") || name == "cluster-reload-options" {
        require(visibleButtons("Close picker").count == 1, "Context picker must be visibly open")
        // The themed backdrop composites over paper; the theme check still applies.
        expected = theme == "dark" ? expected.map { $0 * 0.45 }
            : zip(expected, [CGFloat](repeating: 11, count: 3)).map { $0 * 0.82 + $1 * 0.18 }
    }
    let actual = rgba.prefix(3).map { CGFloat($0) }
    require(zip(actual, expected).allSatisfy { abs($0 - $1) < 8 },
            "Native screenshot does not show the expected \(theme) Orka paper: \(actual)")
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    try VNImageRequestHandler(url: URL(fileURLWithPath: file)).perform([request])
    let observations = request.results ?? []
    let recognized = observations.map { observation in
        "\(observation.boundingBox): \(observation.topCandidates(1).first?.string ?? "")"
    }.joined(separator: "\n")
    try recognized.write(toFile: "\(output)/\(name).ocr.txt", atomically: true, encoding: .utf8)
    if name == "new-agent-probe" || name == "send-probe" || name == "cluster-probe" || name == "overview-probe" || name.hasPrefix("overview-probe-") || name.hasPrefix("overview-fixture-") { return observations }
    if name.hasPrefix("new-agent-") && name.hasSuffix("-end") {
        require(visibleButtons("Create").count == 1 && visibleButtons("Cancel").count == 1, "Creation actions overlap the fixed chrome")
        return observations
    }
    if name.hasPrefix("new-agent-") {
        let expected = name.hasPrefix("new-agent-created") ? "native-helper" : name == "new-agent-repo-created" ? "web-reader" : "New agent"
        let heading: Bool
        if name.hasPrefix("new-agent-repo-") || name.hasSuffix("minimum") {
            // These captures use 900pt content: bind the actual visible page heading,
            // not a normalized band inherited from the 800pt creation screenshots.
            heading = nativeTextBounds(expected, heading: true).count == 1
                && observations.contains { $0.topCandidates(1).first?.string == expected }
        } else {
            heading = observations.contains { observation in
                let top = 1 - observation.boundingBox.maxY
                return top > 0.10 && top < 0.25 && observation.topCandidates(1).first?.string == expected
            }
        }
        require(heading, "Native creation screenshot is missing \(expected)")
        print("PASS \(name): native owned-agent creation, no workspace or model")
        return observations
    }
    if name == "before-settings" || name == "designer-settings" {
        // These captures intentionally scroll the page heading out of view.
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        require(name == "before-settings" ? text.contains("More settings") : text.contains("Personality") && text.contains("Description"),
                "Designer settings section is not visible: \(text)")
        print("PASS \(name): native Designer settings section observed")
        return observations
    }
    if name.hasPrefix("cluster-reload") {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        if name == "cluster-reload-failed" {
            require(text.contains("kubeconfig could not be read") && text.contains("Copy details"), "Reload failure recovery missing: \(text)")
        } else if name == "cluster-reload-options" {
            require(text.contains("fixture-reloaded"), "Corrected kubeconfig context not reread: \(text)")
        } else {
            require(!text.contains("Access confirmed") && !text.contains("Cluster found"), "Reload retained stale selection: \(text)")
        }
        print("PASS \(name): offline native reload, no real credentials")
        return observations
    }
    if name == "cluster-conditional" || name == "cluster-denied" {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        // Vision can split the camel-case label even though the native field is present.
        let accountName = text.contains("ServiceAccount name") || text.contains("Service Account name")
        if name == "cluster-conditional" {
            require(accountName && text.contains("cannot list ServiceAccounts in agents") && text.contains("cluster administrator"), "Forbidden-list name fallback missing: \(text)")
        } else {
            require(text.contains("account and namespace") && text.contains("Access was denied") && !accountName, "Denial incorrectly offered identity substitution: \(text)")
            guard let explanation = nativeTextBounds("Access was denied").first,
                let retry = nativeButtons("Check again").compactMap({ nativeBounds($0) }).first else {
                fail("Access error or its retry action is missing")
            }
            require(explanation.maxY <= retry.minY, "Access error must appear above Check again")
        }
        print("PASS \(name): native fixture access state, no real credentials")
        return observations
    }
    if let expected = ["send-ready": "Ready to send", "send-blocked": "Blocked", "send-cannot-check": "Bridge cannot check"][name] {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        require(text.contains(expected), "Native preflight evidence missing: \(expected); observed: \(text)")
        print("PASS \(name): observed native preflight evidence; no workload submitted")
        return observations
    }
    if name == "send-history-restarted" {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        // Vision can misread small dark badges; exact visible AX text proves the same property.
        let savedPass = text.contains("Saved tests: pass") || nativeTextIsVisible("Saved tests: pass")
        let savedUnknown = text.contains("Saved tests: unknown") || nativeTextIsVisible("Saved tests: unknown")
        require(text.contains("Past Sends") && savedPass && savedUnknown, "Newest saved results missing after restart: \(text)")
        require(text.contains("bridge-run-") && text.contains("fixture-secondary"), "Saved destination or planned Agent missing: \(text)")
        print("PASS send-history-restarted: saved summaries after app restart; no new Send")
        return observations
    }
    if name == "send-history-passed" {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        require(text.contains("Saved tests: pass"), "Earlier saved pass missing: \(text)")
        return observations
    }
    if name == "send-history-answer" {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        require(text.contains("Saved on this computer") && text.contains("Fixture answer:"), "Offline saved answer missing after restart: \(text)")
        print("PASS send-history-answer: durable answer opened offline after restart, no cluster access restored")
        return observations
    }
    if name.hasPrefix("send-") {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        // A raw navigation probe can land below the card after reflow. It guides scrolling, not proof.
        if searching && name == "send-probe" { return observations }
        require(sendFrameVisible(observations), "Send panel not visible: \(text)")
        print("PASS \(name): fixture-backed native Send screen; no real cluster or model")
        return observations
    }
    if name.hasPrefix("cluster-success") || name.hasPrefix("cluster-details") || name.hasPrefix("cluster-selected-list") {
        // Scrolled matrix frames prove their own visible content, not an off-screen page heading.
        if name.hasPrefix("cluster-selected-list") {
            require(nativeTextIsVisible(name.hasSuffix("-end") ? "fixture-secondary" : "Choose where agents run"),
                    "Selected-list matrix anchor is not visible")
        } else if name.hasPrefix("cluster-details") {
            require(nativeTextIsVisible("Orka image"), "Open cluster details are not visible")
        } else if name.hasSuffix("-end") {
            require(visibleButtons("Done").count == 1 && visibleButtons("Open your agent").count == 1,
                    "Cluster handoff actions are not visible")
        } else {
            require(nativeTextIsVisible("fixture-secondary"), "Connected cluster name is not visible")
        }
        return observations
    }
    if name.hasPrefix("cluster-access") {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        require(text.contains("Access confirmed") && text.contains("Open your agent") && !nativeButtons("Done").isEmpty,
                "Fixture access must finish with Done and an optional agent handoff: \(text)")
        return observations
    }
    if name.hasPrefix("cluster-card") {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        // The image is folded into Details until the user opens it.
        if name == "cluster-card-details" {
            require(text.contains("orka:fixture"), "Fixture Orka image is not visible in Details: \(text)")
        } else {
            require(text.contains("Cluster found") && !text.contains("Access confirmed"), "Fixture Orka not visible: \(text)")
        }
        print("PASS \(name): native discovery card; no Use or Send")
        return observations
    }
    if name.hasPrefix("cluster-connect-known") {
        require(nativeTextIsVisible("Cluster found") && !nativeButtons("Use this cluster").isEmpty,
                "Remembered Connect did not discover its known context without choosing again")
        require(!nativeStrings().contains { $0.contains("choose its context to check it again") },
                "Remembered Connect still asks for its context")
        return observations
    }
    if name.hasPrefix("cluster-add") {
        // Add opens a blank context choice and keeps the Designer destination,
        // so only a confirmation inside the page body would make this entry non-blank.
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        let headings = nativeTextBounds("Add a cluster", heading: true)
        require(headings.count == 1, "Add a cluster heading must be visible")
        let frame = window()!.1
        func centre(_ row: VNRecognizedTextObservation) -> CGPoint {
            CGPoint(x: frame.minX + row.boundingBox.midX * frame.width,
                    y: frame.minY + (1 - row.boundingBox.midY) * frame.height)
        }
        // Header AT descriptions and popup AX values are not proof of rendered page text.
        let bodyConfirmed = observations.contains {
            $0.topCandidates(1).first?.string.contains("Access confirmed") == true
                && bodyBounds().contains(centre($0)) && centre($0).y >= headings[0].maxY
        }
        let pickers = visibleButtons("kubectl context")
        let blank = pickers.count == 1 && observations.contains {
            $0.topCandidates(1).first?.string.contains("Choose a context") == true && pickers[0].contains(centre($0))
        }
        let checks = ["back": text.contains("Back to clusters"), "no discovery": !text.contains("Cluster found"),
            "no body confirmation": !bodyConfirmed, "no open": nativeButtons("Open your agent").isEmpty,
            "no recheck": nativeButtons("Check access again").isEmpty, "blank picker": blank]
        require(checks.values.allSatisfy { $0 }, "Add a cluster did not open a blank Places subview: \(checks): \(text)")
        print("PASS \(name): native Places subview with blank kubectl context")
        return observations
    }
    if name.hasPrefix("cluster-manage") {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        let backMiss = ["Pack to clusters", "back to clusters"].contains(where: text.contains)
        require(text.contains("Back to clusters") || backMiss && visibleButtons("Back to clusters").count == 1,
                "Manage is not a Places subview: \(text)")
        if name == "cluster-manage-change" { // Change context only reveals the select.
            require(text.contains("kubectl context") || nativeTextIsVisible("kubectl context"), "Change context did not reveal the context choice: \(text)")
        } else {
            require(!nativeButtons("Open your agent").isEmpty && !nativeButtons("Done").isEmpty
                && !nativeButtons("Show details").isEmpty && nativeStrings().contains { $0 == "AI connection" }
                && nativeStrings().contains { $0.contains("Agents on this cluster") }
                && nativeStrings().contains { $0.contains("runtime-reader") }
                && nativeStrings().contains { $0.contains("Checked") }
                && ["Change context", "Check access again", "Reload kubeconfig", "Remove"].allSatisfy { nativeButtons($0).isEmpty },
                "Manage lost its summary or exposed maintenance outside Show details")
        }
        print("PASS \(name): native Manage for the selected cluster")
        return observations
    }
    if name == "cluster-remove-confirmation" {
        require(nativeButtons("Remove cluster").count == 1 && nativeButtons("Keep cluster").count == 1,
                "Remove needs an explicit confirmation and a way to keep the cluster")
        return observations
    }
    if page == "Clusters" {
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        // Vision can read the capital I in AI as lowercase l. Match the rest of the heading.
        let welcome = text.contains("What do you want done") || !nativeButtons("See an example first").isEmpty
        let places = text.contains("Choose where agents run") || nativeTextIsVisible("Choose where agents run")
        let detail = text.contains("Back to clusters") || text.contains("Where should your agents run?") || !nativeButtons("Continue").isEmpty
        require(welcome || places || detail, "Places heading or subview not visible; observed: \(text)")
        require(!text.contains("Engine diagnostics"), "Removed engine diagnostics is still visible: \(text)")
        if places && !detail {
            // Places keeps direct row actions; the local subview owns the setup choices.
            require(!text.contains("Where should your agents run") && !text.contains("On a cluster you already have"),
                    "Setup chooser leaked onto Places: \(text)")
        }
        if name.hasPrefix("local-preflight") {
            let radios = nativeElements().filter { attribute($0, "AXRole") as? String == "AXRadioButton" }
            for choice in ["On this computer", "On a cluster you already have"] {
                require(radios.contains { element in
                    ["AXTitle", "AXDescription"].contains { (attribute(element, $0) as? String)?.hasPrefix(choice) == true }
                }, "Missing local setup choice: \(choice)")
            }
            require(nativeStrings().contains { $0.contains("Step 1 of 4") } && nativeButtons("Continue").count == 1,
                    "Local setup choice lost its step counter or Continue")
        }
        if !welcome {
            require(!nativeTextIsVisible("Workspace setup is not included in this build. Nothing will be downloaded or started."),
                    "Supported release still reports setup unavailable")
        }
        if ["places-empty", "places-after-form", "places-local", "places-retained"].contains(name) {
            // An empty session still shows this computer and one direct next action.
            require(places && !welcome && !detail && nativeTextIsVisible("No other clusters added yet")
                && visibleButtons("Set up").count == 1 && nativeButtons("Add a cluster").count == 1,
                "Empty Places did not retain this computer, its Set up action and Add a cluster")
            requireNoLocalWorkspace()
            require(!observations.contains {
                let label = $0.topCandidates(1).first?.string ?? ""
                return label.contains("Connect the AI") || label.contains("Connect the Al")
            }, "Model connection is exposed before workspace readiness")
        }
        print("PASS \(name): native workspace screen observed")
        return observations
    }
    let pages = ["Your agents", "Designer", "Clusters", "web-reader"]
    let headings = Set(observations.compactMap { observation -> String? in
        let top = 1 - observation.boundingBox.maxY
        guard top > 0.10 && top < 0.25,
            let text = observation.topCandidates(1).first?.string,
            pages.contains(text) else { return nil }
        return text
    })
    // Negative control: persistent nav labels must not qualify as page headings.
    let expectedHeading = page == "Designer" ? "web-reader" : page == "Overview" ? "Your agents" : page
    for wrong in pages where wrong != expectedHeading && !(page == "Designer" && wrong == "Designer") {
        require(!headings.contains(wrong), "Unexpected \(wrong) page heading; expected \(page)")
    }
    require(headings == Set([expectedHeading]), "Native screenshot does not show the expected \(expectedHeading) page heading; observed \(headings)")
    print("PASS \(name): native \(page) heading observed")
    return observations
}
func scrollMain(_ pixels: Int32) {
    let rect = window()!.1
    let scroll = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: pixels, wheel2: 0, wheel3: 0)
    scroll?.location = CGPoint(x: rect.maxX - 20, y: rect.midY)
    scroll?.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.3)
}
func setContent(_ width: CGFloat, _ height: CGFloat) {
    guard let nativeWindow = nativeElements().first(where: { attribute($0, "AXRole") as? String == "AXWindow" }) else {
        fail("Cannot find the current native window")
    }
    let frame = NSWindow.frameRect(forContentRect: CGRect(x: 0, y: 0, width: width, height: height),
        styleMask: [.titled, .closable, .miniaturizable, .resizable])
    var size = frame.size
    require(AXUIElementSetAttributeValue(nativeWindow, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!) == .success, "Cannot resize native window")
    Thread.sleep(forTimeInterval: 0.5)
    require(abs(window()!.1.width - size.width) < 3 && abs(window()!.1.height - size.height) < 3, "Native \(width)x\(height) content area not verified")
}
func ocrBounds(_ observation: VNRecognizedTextObservation, in frame: CGRect) -> CGRect {
    let box = observation.boundingBox
    return CGRect(x: frame.minX + box.minX * frame.width, y: frame.minY + (1 - box.maxY) * frame.height,
                  width: box.width * frame.width, height: box.height * frame.height)
}
func uniqueOCRTarget(_ boxes: [CGRect], within viewport: CGRect) -> CGRect? {
    let matches = boxes.filter { viewport.contains($0) }
    guard let first = matches.first else { return nil }
    return matches.allSatisfy({ $0 == first }) ? first : nil
}
// Exercise the exact native selection gate, including the duplicate local/history identity.
do {
    let viewport = CGRect(x: 0, y: 0, width: 100, height: 100)
    let one = CGRect(x: 10, y: 10, width: 20, height: 10)
    let other = CGRect(x: 10, y: 50, width: 20, height: 10)
    let outside = CGRect(x: 120, y: 10, width: 20, height: 10)
    require(uniqueOCRTarget([one], within: viewport) == one, "OCR preserves one in-region target")
    require(uniqueOCRTarget([outside, one], within: viewport) == one, "OCR excludes outside-region duplicates")
    require(uniqueOCRTarget([one, other], within: viewport) == nil, "OCR rejects distinct in-region targets")
    let overlap = CGRect(x: 15, y: 10, width: 20, height: 10)
    require(uniqueOCRTarget([one, overlap], within: viewport) == nil, "OCR rejects partial-overlap targets")
    require(uniqueOCRTarget([one, one], within: viewport) == one, "OCR coalesces one repeated hit region")
    require(uniqueOCRTarget([outside], within: viewport) == nil, "OCR rejects outside-region targets")
}
func clickSendControl(_ label: String, scrolling pixels: Int32, region: String? = "Send and cluster tests") throws {
    let root = region.map { nativeRegion($0) }
    if ["Check again", "Send as a separate agent", "See in Overview", "Run cluster tests again", "Send and run tests", "Update agent and run tests"].contains(label),
        nativeButtons(label, root: root).allSatisfy({ nativeBounds($0).map { $0.width <= 1 || $0.height <= 1 } ?? true }) {
        // Extra actions now live in the existing resources disclosure. Open it
        // explicitly rather than clicking a hidden control or emulating layout.
        try clickSendControl("Details", scrolling: pixels, region: region)
    }
    // Disabling Start during IPC can reset WebKit's keyboard focus. Locate the
    // real control after each state transition instead of counting Tab stops.
    var observed: [String] = []
    var disabled = false
    for _ in 0..<4 {
        // A state capture may leave the card header above the viewport. Locate
        // its actual button instead of searching only in the caller's direction.
        let buttons = label == "Details" ? nativeButtons(label, root: root).filter { element in
            ["AXTitle", "AXDescription"].contains { (attribute(element, $0) as? String)?.contains("Resources and file hashes") == true }
        } : nativeButtons(label, root: root)
        if buttons.count == 1, let bounds = nativeBounds(buttons[0]), !bodyBounds().contains(bounds) {
            scrollMain(Int32((bodyBounds().minY + 32 - bounds.minY).rounded()))
        }
        // Shared disclosures expose real button semantics. Use their enabled,
        // visible hit region instead of OCR-ing the decorative chevron as text.
        if buttons.count == 1, attribute(buttons[0], "AXEnabled") as? Bool == true,
            let bounds = nativeBounds(buttons[0]), bodyBounds().contains(bounds) {
            let point = CGPoint(x: bounds.midX, y: bounds.midY)
            mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
            Thread.sleep(forTimeInterval: 0.5)
            return
        }
        let seen = try capture("send-probe", "Designer", searching: true)
        observed = seen.compactMap { $0.topCandidates(1).first?.string }
        let frame = window()!.1
        let viewport: CGRect?
        if let root {
            guard let bounds = nativeBounds(root) else { fail("Native OCR click region has no bounds: \(region ?? "")") }
            viewport = bounds.intersection(bodyBounds())
        } else { viewport = nil }
        let scopedSeen = seen.filter { viewport?.contains(ocrBounds($0, in: frame)) ?? true }
        // Vision includes the native disclosure triangle as a bullet in this label.
        let matches = scopedSeen.filter { observation in
            guard let text = observation.topCandidates(1).first?.string else { return false }
            return text == label || (label == "published-checksum" && text.range(of: #"published-checksum\b"#, options: [.regularExpression, .caseInsensitive]) != nil) || (label == "Details" && text.lowercased().range(of: #"^\W*details\b"#, options: .regularExpression) != nil) || (label == "Ready to send" && text.range(of: #"^\W*Ready to send\b"#, options: .regularExpression) != nil)
        }
        if let target = matches.first, let control = target.topCandidates(1).first?.string {
            if let viewport {
                require(uniqueOCRTarget(matches.map { ocrBounds($0, in: frame) }, within: viewport) != nil,
                        "Ambiguous native OCR control in \(region ?? ""): \(label)")
            }
            if ["Send and run tests", "Update agent and run tests", "Run cluster tests again", "Send as a separate agent", "Cancel tests", "Retry cancellation", "Check cluster status", "Check again", "Reload saved files", "Send to a cluster", "See in Overview"].contains(label) {
                let enabled = nativeElements(root).contains { element in
                    attribute(element, "AXRole") as? String == "AXButton"
                        && attribute(element, "AXEnabled") as? Bool == true
                        && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == label })
                }
                if !enabled { disabled = true; Thread.sleep(forTimeInterval: 0.3); continue }
            }
            clickText(control, in: [target])
            return
        }
        scrollMain(pixels)
    }
    fail("Send control \(disabled ? "stayed disabled" : "not visible"): \(label); observed: \(observed)")
}
func requirePlaceCards() {
    // Counted over the whole page; cards can be below the fold. The checked primary offers Use,
    // the selected secondary only Manages, and local still needs Set up.
    let strings = nativeStrings()
    for expected in ["fixture-cluster", "fixture-secondary", "Checked", "In use", "Orka in"] {
        require(strings.contains { $0.contains(expected) }, "Places is missing \(expected)")
    }
    require(!strings.contains { $0.contains("No other clusters added yet") } && nativeButtons("Use").count == 1
        && nativeButtons("Manage").count == 1 && nativeButtons("Set up").count == 1 && nativeButtons("Add a cluster").count == 1,
        "expected one Use, Manage, Set up and Add a cluster")
    print("PASS: Places shows last-check times and current selection with one action per card")
}
func localPreflight(_ name: String) throws {
    clickButton("Set up")
    scrollMain(1000)
    try capture(name, "Clusters")
    requireNoLocalWorkspace() // the size screen must never start the VM
    require(visibleButtons("Back to clusters").isEmpty, "Local setup must not have a header Back link")
    clickTab("Clusters", [])
}
func requireBlankPair(_ agent: URL, _ tests: URL, _ name: String) throws {
    let content = try String(contentsOf: agent, encoding: .utf8)
    let parts = content.components(separatedBy: "---")
    require(parts.count == 3 && parts[2].trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        && !content.contains("web_fetch") && !content.contains("tools:"), "Created draft must have empty instructions and no tools")
    let expected = "---\nschemaVersion: 1\n---\n\n# \(name) tests\n\n```yaml agent-test\nid: first-test\ninput: \"\"\nassertions: []\n```"
    let checks = try String(contentsOf: tests, encoding: .utf8).replacingOccurrences(of: "\r\n", with: "\n")
    require(checks.trimmingCharacters(in: .whitespacesAndNewlines) == expected, "Created empty test skeleton differs from the draft contract")
}
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
setContent(1280, 800)
var initial = try capture("initial", "Clusters")
// Each theme owns a fresh runner profile; verify retention after its own restart.
let initiallyWelcome = initial.contains { $0.topCandidates(1).first?.string.contains("What do you want done") == true }
    || !nativeButtons("See an example first").isEmpty
require(initiallyWelcome, "Expected a first-run welcome in the fresh \(theme) job")
for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
    setContent(width, height)
    scrollMain(4000)
    try capture("welcome-\(size)", "Clusters")
    scrollMain(-4000)
    try capture("welcome-\(size)-end", "Clusters")
    require(visibleButtons("See an example first").count == 1,
            "Job intake example route must remain reachable at \(size)")
    let submit = nativeButtons("Save job and set up")
    require(submit.count == 1, "Job intake needs one exact native Save job and set up control at \(size)")
    require(submit.allSatisfy { attribute($0, "AXEnabled") as? Bool == false },
            "Blank job submission must stay disabled")
    require(nativeTextIsVisible("NOTHING LEAVES THIS COMPUTER DURING SETUP. NO ACCOUNT IS CREATED."),
            "Job intake privacy contract must remain visible at \(size)")
    // A release build names its download; the bare fallback means the sizes never arrived.
    let sizedLine = { nativeStrings().contains { $0.range(of: #"Mostly a \d+\.\d GB download"#, options: .regularExpression) != nil } }
    let sizedDeadline = Date(timeIntervalSinceNow: 10)
    while !sizedLine() && Date() < sizedDeadline { Thread.sleep(forTimeInterval: 0.5) }
    require(sizedLine(), "Job intake lost its measured download size at \(size)")
    requireNoLocalWorkspace()
}
setContent(1280, 800)
scrollMain(4000)
if initiallyWelcome {
    clickButton("See an example first")
    clickButton("Clusters")
    initial = try capture("places-empty", "Clusters")
}
try localPreflight("local-preflight")
do {
    clickTab("Designer", initial)
    try capture("agent-before-creation", "Designer")
    clickButton("New agent")
    Thread.sleep(forTimeInterval: 0.3)
    for character in "native-helper".utf16 {
        var unit = character
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
            event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
            event.flags = []; event.post(tap: .cghidEventTap)
        }
    }
    // Blur the identifier before submission so macOS text correction cannot hide in focus.
    key(48)
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        setContent(width, height); scrollMain(4000)
        try capture("new-agent-form-\(size)", "Designer")
        if size == "minimum" {
            scrollMain(-4000)
            try capture("new-agent-form-\(size)-end", "Designer")
        }
    }
    setContent(1280, 800); scrollMain(4000)
    let creationForm = try capture("new-agent-form", "Designer")
    clickText("Create", in: creationForm)
    let folder = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Orka/agents/native-helper")
    let agent = folder.appendingPathComponent("native-helper.agent.md")
    let checks = folder.appendingPathComponent("native-helper.agent.test.md")
    for _ in 0..<50 {
        if FileManager.default.fileExists(atPath: checks.path) && nativeButtons("Create").isEmpty && nativeTextIsVisible("Instructions") { break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    if !FileManager.default.fileExists(atPath: agent.path) || !FileManager.default.fileExists(atPath: checks.path) {
        let observed = try capture("new-agent-probe", "Designer")
        let text = observed.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        fail("Creation did not publish its pair; native screen: \(text)")
    }
    require(try String(contentsOf: agent, encoding: .utf8).contains("name: \"native-helper\"\n"), "Created agent metadata missing")
    try requireBlankPair(agent, checks, "native-helper")
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        setContent(width, height); scrollMain(4000)
        try capture("new-agent-created-\(size)", "Designer")
    }
    setContent(1280, 800); scrollMain(4000)
    let created = try capture("new-agent-created", "Designer")
    requireNoLocalWorkspace()
    clickTab("Overview", created)
    for _ in 0..<30 {
        if !nativeButtons("Open web-reader").isEmpty { break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    clickButton("Open web-reader")
    for _ in 0..<30 {
        if nativeTextIsVisible("web-reader") { break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    try capture("agent-after-creation", "Designer")
}
do {
    // The owned form works before any workspace exists. Exercise its real writer,
    // not injected renderer data; editing remains owned during setup too.
    // Focus after Places differs by entry; the keyboard traversal below sets its own start.
    clickTab("Designer", initial)
    try capture("agent-form", "Designer")
    let instructions = nativeElements().filter { attribute($0, "AXRole") as? String == "AXTextArea"
        && ((attribute($0, "AXTitle") as? String == "Instructions") || (attribute($0, "AXDescription") as? String == "Instructions")) }
    require(instructions.count == 1, "Cannot identify the owned Instructions field")
    guard let fieldBounds = nativeBounds(instructions[0]) else { fail("Instructions field bounds missing") }
    let rect = window()!.1
    let visibleField = fieldBounds.intersection(rect)
    require(!visibleField.isEmpty && !visibleField.isNull, "Instructions field is not visible")
    let owned = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Orka/agents/web-reader/web-reader.agent.md")
    let before = try String(contentsOf: owned, encoding: .utf8)
    let fieldPoint = CGPoint(x: visibleField.midX, y: visibleField.midY)
    mouse(.leftMouseDown, fieldPoint); mouse(.leftMouseUp, fieldPoint)
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 125, keyDown: down)!
        event.flags = .maskCommand; event.post(tap: .cghidEventTap)
    }
    let suffix = " Native form probe \(UUID().uuidString)."
    for character in suffix.utf16 {
        var unit = character
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
            event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
            // Do not inherit Command from the caret-navigation events still in flight.
            event.flags = []
            event.post(tap: .cghidEventTap)
        }
    }
    var saved = false
    for _ in 0..<20 {
        if (try String(contentsOf: owned, encoding: .utf8)) == before + suffix { saved = true; break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(saved, "Native form did not preserve the file and append the exact instructions suffix")
    let savedForm = try capture("agent-form-saved", "Designer")
    let savedFooter = savedForm.filter { $0.boundingBox.maxY < 0.1 }
        .compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
    require(savedFooter.range(of: "\\bSaved\\b", options: .regularExpression) != nil
        && !savedFooter.localizedCaseInsensitiveContains("not saved"),
        "Form save was not acknowledged in footer: \(savedFooter)")
    clickTab("Clusters", savedForm)
    initial = try capture("places-after-form", "Clusters")
    // This is a real release with an active image pin. Leave Continue untouched;
    // native lifecycle/model fixtures cover setup without a VM separately.
}
try capture("places-local", "Clusters")
app.terminate()
for _ in 0..<30 {
    if app.isTerminated { break }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
}
require(app.isTerminated, "Bridge did not quit before preference check")
app = try launch()
app.activate(options: [])
waitForWorkspaceRenderer()
let retained = try capture("places-retained", "Clusters")
require(nativeButtons("See an example first").isEmpty,
        "Restart must retain welcome dismissal without starting a workspace")
clickTab("Overview", retained)
_ = try capture("overview", "Overview")
require(!nativeTextIsVisible("This workspace can't list its agents yet."),
        "Saved files must not be contradicted by a local inventory caveat")
require(!visibleButtons("Open web-reader").isEmpty && nativeTextIsVisible("web-reader"),
        "Overview must show the saved file opened through the native Designer")
func activateOverviewButton(_ label: String) throws {
    let buttons = nativeButtons(label).filter { attribute($0, "AXEnabled") as? Bool == true }
        .filter { nativeBounds($0).map(bodyBounds().contains) == true }
    require(buttons.count == 1, "Cannot identify keyboard Overview control \(label)")
    require(AXUIElementSetAttributeValue(buttons[0], kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success,
            "Cannot focus Overview control \(label)")
    key(36)
    Thread.sleep(forTimeInterval: 0.5)
}
try activateOverviewButton("Show places for web-reader")
_ = try capture("overview-expanded-keyboard", "Overview")
require(nativeTextIsVisible("This computer"), "Keyboard disclosure did not reveal Overview places")
try activateOverviewButton("Open web-reader")
let designer = try capture("designer-opened-from-overview-keyboard", "Designer")
require(designer.contains { ($0.topCandidates(1).first?.string ?? "") == "web-reader" },
        "Keyboard Open did not retain the saved agent destination")
let clustersTab = clickTab("Clusters", designer)
try capture("clusters-pointer", "Clusters")
// Pointer activation on macOS need not focus a button. Establish and verify the
// native keyboard starting point, then exercise actual traversal/activation.
require(AXUIElementSetAttributeValue(clustersTab, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success,
        "Cannot focus Clusters for native keyboard traversal")
var focused: CFTypeRef?
require(AXUIElementCopyAttributeValue(clustersTab, kAXFocusedAttribute as CFString, &focused) == .success
    && focused as? Bool == true, "Clusters did not receive native keyboard focus")
// From Clusters, Designer and Overview are the preceding navigation stops.
key(48, shift: true); key(48, shift: true); key(49)
try capture("overview-keyboard", "Overview")
key(48); key(49); try capture("designer-keyboard", "Designer")
key(48); key(36); try capture("clusters-keyboard", "Clusters")
// The same owned file stays editable before workspace startup.
let clusters = try capture("before-file", "Clusters")
clickTab("Designer", clusters)
let agent = try capture("agent-view", "Designer")
guard let fileButton = agent.first(where: { $0.topCandidates(1).first?.string == "File" }) else {
    fail("File view switch is not visible")
}
let fileRect = window()!.1
let filePoint = CGPoint(x: fileRect.minX + fileButton.boundingBox.midX * fileRect.width,
                        y: fileRect.minY + (1 - fileButton.boundingBox.midY) * fileRect.height)
mouse(.leftMouseDown, filePoint); mouse(.leftMouseUp, filePoint)
Thread.sleep(forTimeInterval: 0.5)
let fileView = try capture("file-view", "Designer")
require(!fileView.contains { $0.topCandidates(1).first?.string == "Read-only during setup" },
        "Owned File view is still labelled read-only")
let ownedFile = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Orka/agents/web-reader/web-reader.agent.md")
let bytesBeforeEdit = try String(contentsOf: ownedFile, encoding: .utf8)
let editors = nativeElements().filter { attribute($0, "AXRole") as? String == "AXTextArea"
    && ((attribute($0, "AXTitle") as? String == "Agent file") || (attribute($0, "AXDescription") as? String == "Agent file")) }
require(editors.count == 1, "Cannot identify the owned native File editor")
guard let editorBounds = nativeBounds(editors[0]) else { fail("Native File editor bounds missing") }
let visibleEditor = editorBounds.intersection(fileRect)
require(!visibleEditor.isEmpty && !visibleEditor.isNull, "Native File editor is offscreen")
let editorPoint = CGPoint(x: visibleEditor.midX, y: visibleEditor.midY)
mouse(.leftMouseDown, editorPoint); mouse(.leftMouseUp, editorPoint)
Thread.sleep(forTimeInterval: 0.2)
require(attribute(editors[0], "AXFocused") as? Bool == true, "Native File editor did not receive focus")
for down in [true, false] {
    let event = CGEvent(keyboardEventSource: nil, virtualKey: 125, keyDown: down)!
    event.flags = .maskCommand; event.post(tap: .cghidEventTap)
}
let marker = " Native setup edit probe \(UUID().uuidString)."
for character in marker.utf16 {
    var unit = character
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
        event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
        event.flags = []; event.post(tap: .cghidEventTap)
    }
}
var editSaved = false
for _ in 0..<20 {
    if (try String(contentsOf: ownedFile, encoding: .utf8)) == bytesBeforeEdit + marker { editSaved = true; break }
    Thread.sleep(forTimeInterval: 0.1)
}
require(editSaved, "Setup edit did not append the exact bytes to the owned file")
scrollMain(1000)
let editedFile = try capture("file-edited", "Designer")
let savedFooter = editedFile.filter { $0.boundingBox.maxY < 0.1 }
    .compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
require(savedFooter.range(of: "\\bSaved\\b", options: .regularExpression) != nil
    && !savedFooter.localizedCaseInsensitiveContains("not saved"), "Setup file save not acknowledged: \(savedFooter)")
print("PASS owned setup editor: native keyboard append persisted exact bytes, without a ready VM")
// Exercise the real settings disclosure; reveal controls are observed, not
// activated, so the walk never opens Finder or touches the clipboard.
clickDesignerView("Agent")
scrollMain(-400)
let beforeSettings = try capture("before-settings", "Designer")
clickText("More settings", in: beforeSettings)
try capture("designer-settings", "Designer")
scrollMain(1000)
let settingsTop = try capture("designer-settings-top", "Designer")
clickTab("Clusters", settingsTop)
// Resize this CI-owned window using its native bottom-right handle.
let rect = window()!.1
// The display may constrain the initial size. Derive the minimum frame from
// AppKit's content-to-frame conversion, not the requested initial dimensions.
let minimumFrame = NSWindow.frameRect(
    forContentRect: CGRect(x: 0, y: 0, width: 540, height: 640),
    styleMask: [.titled, .closable, .miniaturizable, .resizable])
let start = CGPoint(x: rect.maxX - 10, y: rect.maxY - 5)
let end = CGPoint(x: rect.minX + minimumFrame.width - 10, y: rect.minY + minimumFrame.height - 5)
mouse(.leftMouseDown, start)
for step in 1...20 {
    let t = CGFloat(step) / 20
    mouse(.leftMouseDragged, CGPoint(x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t))
    Thread.sleep(forTimeInterval: 0.02)
}
mouse(.leftMouseUp, end)
Thread.sleep(forTimeInterval: 0.5)
let minimum = window()!.1
require(abs(minimum.width - minimumFrame.width) < 3 && abs(minimum.height - minimumFrame.height) < 3, "Native minimum-size resize not verified: \(minimum), expected \(minimumFrame.size)")
try capture("clusters-minimum", "Clusters")
// Set the CI-owned native content area to 540x900, not a browser viewport.
let application = AXUIElementCreateApplication(app.processIdentifier)
var windowsValue: CFTypeRef?
require(AXUIElementCopyAttributeValue(application, kAXWindowsAttribute as CFString, &windowsValue) == .success,
        "Cannot read native windows for compact walk")
let nativeWindow = (windowsValue as! [AXUIElement])[0]
var origin = CGPoint(x: 20, y: 40)
require(AXUIElementSetAttributeValue(nativeWindow, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &origin)!) == .success, "Cannot position compact window")
let compactFrame = NSWindow.frameRect(forContentRect: CGRect(x: 0, y: 0, width: 540, height: 900),
    styleMask: [.titled, .closable, .miniaturizable, .resizable])
var compactSize = compactFrame.size
require(AXUIElementSetAttributeValue(nativeWindow, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &compactSize)!) == .success, "Cannot resize compact window")
Thread.sleep(forTimeInterval: 0.5)
require(abs(window()!.1.width - compactSize.width) < 3 && abs(window()!.1.height - compactSize.height) < 3,
        "Native 540x900 content area not verified: \(window()!.1)")
let compactClusters = try capture("clusters-compact", "Clusters")
try localPreflight("local-preflight-compact")
clickTab("Designer", compactClusters)
try capture("designer-compact", "Designer")
clickDesignerView("File")
require(nativeElements().contains { attribute($0, "AXRole") as? String == "AXTextArea"
    && attribute($0, "AXTitle") as? String == "Agent file" }, "Compact File editor did not open")
let compactFile = try capture("file-compact", "Designer")
clickTab("Clusters", compactFile)
print("PASS native walk: pointer, Shift-Tab/Tab, Space, Enter and minimum window")
if ProcessInfo.processInfo.environment["BRIDGE_WALK_KUBECONFIG"] != nil {
    @discardableResult
    func fixtureControl(_ value: [String: Any]) throws -> [String: Any] {
        let config = try Data(contentsOf: URL(fileURLWithPath: ProcessInfo.processInfo.environment["BRIDGE_WALK_KUBECONFIG"]!))
        let parsed = try JSONSerialization.jsonObject(with: config) as! [String: Any]
        let clusters = parsed["clusters"] as! [[String: Any]]
        let cluster = clusters[0]["cluster"] as! [String: Any]
        guard let server = URLComponents(string: cluster["server"] as! String), server.scheme == "http",
            server.host == "127.0.0.1", server.port != nil, server.user == nil, server.password == nil,
            server.path.isEmpty, server.query == nil, server.fragment == nil else {
            fail("Fixture controls require the CI loopback server")
        }
        let command = Process()
        command.executableURL = URL(fileURLWithPath: "/usr/bin/curl")
        command.arguments = ["--noproxy", "*", "--fail", "--silent", "--show-error", "--max-time", "5",
            "-H", "Content-Type: application/json", "--data-binary",
            String(data: try JSONSerialization.data(withJSONObject: value), encoding: .utf8)!,
            "http://127.0.0.1:\(server.port!)/fixture-control"]
        let reply = Pipe(); command.standardOutput = reply
        try command.run()
        let data = reply.fileHandleForReading.readDataToEndOfFile()
        command.waitUntilExit()
        require(command.terminationStatus == 0, "Fixture control failed: \(value)")
        return try JSONSerialization.jsonObject(with: data) as! [String: Any]
    }
    // CI runs both themes against one fixture process; keep each Send sequence deterministic.
    try fixtureControl(["action": "reset"])
    try capture("before-cluster", "Clusters")
    // No local selection exists in this no-VM walk. Add a cluster offers only an existing cluster.
    clickButton("Add a cluster")
    Thread.sleep(forTimeInterval: 1)
    // Context choices are ordinary buttons in the accessible popover, not native select options.
    clickButton("kubectl context")
    try capture("cluster-picker-open-compact", "Clusters")
    clickButton("fixture-cluster")
    Thread.sleep(forTimeInterval: 2)
    // Capture discovery before the explicit access confirmation.
    try capture("cluster-card-compact", "Clusters")
    // The image is folded into Details; open it to keep observing the fixture image.
    var detailsOpened = false
    for _ in 0..<4 {
        let seen = try capture("cluster-probe", "Clusters")
        if let label = seen.compactMap({ $0.topCandidates(1).first?.string }).first(where: {
            $0.lowercased().range(of: #"^\W*(show |hide )?details\b"#, options: .regularExpression) != nil
        }) {
            clickText(label, in: seen)
            try capture("cluster-card-details", "Clusters")
            detailsOpened = true
            break
        }
        scrollMain(-200)
    }
    require(detailsOpened, "Discovered Orka has no visible Details control")
    // Keep the current picker while a result stage shows it; otherwise reopen Add a cluster.
    func showContextSelect() {
        scrollMain(1000)
        if nativeTextIsVisible("kubectl context") { return }
        if !visibleButtons("Back to clusters").isEmpty { clickButton("Back to clusters") }
        clickButton("Add a cluster")
        scrollMain(1000)
    }
    // Exercise the existing conditional field and a denial, without issuing a token.
    func chooseFixture(_ index: Int) throws -> [VNRecognizedTextObservation] {
        showContextSelect()
        let names = ["fixture-cluster", "conditional-fixture", "denied-fixture", "missing-tools-fixture", "unreadable-tools-fixture", "fixture-secondary"]
        require((1...names.count).contains(index), "Unknown fixture context index")
        clickButton("kubectl context")
        clickButton(names[index - 1])
        Thread.sleep(forTimeInterval: 1)
        return try capture("cluster-card-selected", "Clusters")
    }
    for (index, name) in [(2, "cluster-conditional"), (3, "cluster-denied")] {
        let selected = try chooseFixture(index)
        clickText("Use this cluster", in: selected)
        Thread.sleep(forTimeInterval: 1)
        scrollMain(-250)
        try capture(name, "Clusters")
    }
    // Change only the CI-owned synthetic kubeconfig, then prove the native session
    // rereads it and invalidates prior access on both failure and correction.
    let configURL = URL(fileURLWithPath: ProcessInfo.processInfo.environment["BRIDGE_WALK_KUBECONFIG"]!)
    let originalConfig = try Data(contentsOf: configURL)
    func reloadFixture() throws {
        // Failed checks mount Reload only inside their expanded error Details.
        if nativeButtons("Reload kubeconfig").isEmpty { clickButton("Details") }
        for _ in 0..<6 {
            let seen = try capture("cluster-probe", "Clusters")
            if seen.contains(where: { $0.topCandidates(1).first?.string == "Reload kubeconfig" }) {
                clickText("Reload kubeconfig", in: seen)
                Thread.sleep(forTimeInterval: 1)
                return
            }
            scrollMain(-200)
        }
        fatalError("Reload kubeconfig control not found")
    }
    do {
        defer { try? originalConfig.write(to: configURL, options: .atomic) }
        try Data("invalid kubeconfig: [".utf8).write(to: configURL, options: .atomic)
        try reloadFixture()
        // Reload resets Details; expose the new configuration error's copy action.
        clickButton("Details")
        try capture("cluster-reload-failed", "Clusters")
        var corrected = try JSONSerialization.jsonObject(with: originalConfig) as! [String: Any]
        var configuredContexts = corrected["contexts"] as! [[String: Any]]
        configuredContexts[0]["name"] = "fixture-reloaded"
        corrected["contexts"] = configuredContexts
        corrected["current-context"] = "fixture-reloaded"
        try JSONSerialization.data(withJSONObject: corrected).write(to: configURL, options: .atomic)
        try reloadFixture()
        showContextSelect()
        try capture("cluster-reload-cleared", "Clusters")
        clickButton("kubectl context")
        try capture("cluster-reload-options", "Clusters")
        clickButton("Close picker")
        try originalConfig.write(to: configURL, options: .atomic)
        try reloadFixture()
    }
    @discardableResult
    func capturePreflight(_ name: String, _ expected: String, anchor: String? = nil,
                          region: String = "Send and cluster tests", requiring: (([VNRecognizedTextObservation]) -> Bool)? = nil) throws -> [VNRecognizedTextObservation] {
        let cropEligible = expected == "published-checksum" || expected == "1 of 1 tests passed"
        func matchesCapturedText(_ value: String) -> Bool {
            let text = normalizedSendText(value)
            guard cropEligible else { return text.localizedCaseInsensitiveContains(expected) }
            let checksumPattern = #"(?<![\w.-])"# + NSRegularExpression.escapedPattern(for: expected) + #"(?![\w.-])"#
            return text.range(of: checksumPattern, options: [.regularExpression, .caseInsensitive]) != nil
        }
        func enlargedSendTextVisible(_ viewport: CGRect, _ frame: CGRect) throws -> Bool {
            guard [viewport.origin.x, viewport.origin.y, viewport.width, viewport.height,
                   frame.origin.x, frame.origin.y, frame.width, frame.height].allSatisfy({ $0.isFinite }),
                !viewport.isEmpty, frame.width > 0, frame.height > 0,
                let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: "\(output)/send-probe.png") as CFURL, nil),
                let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return false }
            let scaleX = CGFloat(image.width) / frame.width, scaleY = CGFloat(image.height) / frame.height
            let pixels = CGRect(x: (viewport.minX - frame.minX) * scaleX,
                y: (viewport.minY - frame.minY) * scaleY,
                width: viewport.width * scaleX, height: viewport.height * scaleY)
            guard [pixels.origin.x, pixels.origin.y, pixels.width, pixels.height].allSatisfy({ $0.isFinite }) else { return false }
            let clipped = pixels.intersection(CGRect(x: 0, y: 0, width: CGFloat(image.width), height: CGFloat(image.height)))
            guard !clipped.isNull, !clipped.isEmpty else { return false }
            // Round inward so the crop never includes pixels outside the visible Send region.
            let rect = CGRect(x: ceil(clipped.minX), y: ceil(clipped.minY),
                width: floor(clipped.maxX) - ceil(clipped.minX), height: floor(clipped.maxY) - ceil(clipped.minY))
            guard rect.width > 0, rect.height > 0, let crop = image.cropping(to: rect),
                let space = CGColorSpace(name: CGColorSpace.sRGB),
                let context = CGContext(data: nil, width: crop.width * 2, height: crop.height * 2,
                    bitsPerComponent: 8, bytesPerRow: 0, space: space,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            context.interpolationQuality = .high
            context.draw(crop, in: CGRect(x: 0, y: 0, width: CGFloat(crop.width * 2), height: CGFloat(crop.height * 2)))
            guard let enlarged = context.makeImage(),
                let png = NSBitmapImageRep(cgImage: enlarged).representation(using: .png, properties: [:]) else { return false }
            let request = VNRecognizeTextRequest()
            request.recognitionLevel = .accurate
            do { try VNImageRequestHandler(cgImage: enlarged, options: [:]).perform([request]) }
            catch { print("Frozen Send crop OCR failed: \(error)"); return false }
            let rows = request.results ?? []
            let text = rows.compactMap { $0.topCandidates(1).first?.string }.reduce("") { text, line in
                text + (text.isEmpty || text.hasSuffix("-") ? "" : " ") + line
            }
            try png.write(to: URL(fileURLWithPath: "\(output)/\(name)-send-crop-2x.png"), options: .atomic)
            try rows.map { "\($0.boundingBox): \($0.topCandidates(1).first?.string ?? "")" }.joined(separator: "\n")
                .write(toFile: "\(output)/\(name)-send-crop-2x.ocr.txt", atomically: true, encoding: .utf8)
            return matchesCapturedText(text)
        }
        for _ in 0..<8 {
            // Preserve a visible transient state before doing any search work.
            let seen = try capture("send-probe", "Designer", searching: true)
            let root = nativeRegion(region)
            guard let bounds = nativeBounds(root), let frame = window()?.1 else {
                fail("Native result region has no capture bounds: \(region)")
            }
            let viewport = bounds.intersection(bodyBounds())
            // Select evidence from this frozen frame, not matching local tests elsewhere in the window.
            let scopedSeen = seen.filter { observation in
                let box = observation.boundingBox
                let textBounds = CGRect(x: frame.minX + box.minX * frame.width,
                    y: frame.minY + (1 - box.maxY) * frame.height,
                    width: box.width * frame.width, height: box.height * frame.height)
                return viewport.contains(textBounds)
            }
            // Code tokens can wrap after a hyphen; do not insert an OCR-only space inside the ID.
            let text = scopedSeen.compactMap { $0.topCandidates(1).first?.string }.reduce("") { text, line in
                text + (text.isEmpty || text.hasSuffix("-") ? "" : " ") + line
            }
            // Section labels are uppercased; Vision also varies casing in the fixed fixture ID.
            // Require Send context in this same captured frame, never later accessibility state.
            var matchesExpected = matchesCapturedText(text)
            // Full-window OCR can omit small IDs or read the count's 1 as l. Re-OCR only these
            // frozen pixels, never correct glyphs; retain the original rows, context and guards.
            let hasContext = (matchesExpected || cropEligible)
                && sendFrameVisible(seen) && (requiring?(seen) ?? true)
            if !matchesExpected && cropEligible && hasContext {
                matchesExpected = try enlargedSendTextVisible(viewport, frame)
            }
            if matchesExpected && hasContext {
                // Keep the frame we verified. A transient state can end between
                // two screenshots; a second OCR pass would test a different frame.
                if name != "send-probe" {
                    for ext in ["png", "ocr.txt"] {
                        try Data(contentsOf: URL(fileURLWithPath: "\(output)/send-probe.\(ext)"))
                            .write(to: URL(fileURLWithPath: "\(output)/\(name).\(ext)"), options: .atomic)
                    }
                }
                print("PASS \(name): frozen native Send frame shows \(expected)")
                return seen
            }
            // Reflow and result updates preserve a scroll offset, not a row.
            // Anchor missing text in either direction, away from fixed chrome.
            // Prefer rendered text over a 1px screen-reader announcement.
            // An explicit anchor is an exact label: the editor's longer storage note is not saved-result provenance.
            // Titles/IDs also occur in local Designer tests; anchor only the named result region.
            let textAnchor = nativeTextBounds(anchor ?? expected, visibleOnly: false, exact: anchor != nil, root: root)
                .filter({ $0.width > 1 && $0.height > 1 })
                .max(by: { $0.width * $0.height < $1.width * $1.height })
            // Action labels are AXButtons, not static text. Closing Details can leave them above the viewport.
            let buttonAnchor = nativeButtons(anchor ?? expected, root: root).compactMap { nativeBounds($0) }
                .first { $0.width > 1 && $0.height > 1 }
            if let anchor = textAnchor ?? buttonAnchor {
                let delta = bodyBounds().minY + 64 - anchor.minY
                if abs(delta) > 2 { scrollMain(Int32(delta.rounded())); continue }
            }
            Thread.sleep(forTimeInterval: 0.3)
        }
        let candidates = nativeElements(nativeRegion(region)).compactMap { element -> String? in
            let values = ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
            guard values.contains(where: { $0.contains(expected) || $0.contains(anchor ?? expected) }) else { return nil }
            let role = attribute(element, "AXRole") as? String ?? "unknown"
            return role + ": " + values.joined(separator: " | ")
        }.prefix(4).joined(separator: "; ")
        fail("Preflight state missing: \(expected); native candidates: \(candidates)")
    }
    let sendActionLabels = ["Cancel tests", "Check cluster status", "Retry cancellation", "Reload saved files", "Choose a cluster", "Check again", "See in Overview", "Fix in Designer", "Send and run tests", "Update agent and run tests", "Run cluster tests again", "Send as a separate agent"]
    func closeSendDetails() throws {
        if nativeButtons("Copy Planned agent", root: nativeRegion("Send and cluster tests")).contains(where: { nativeBounds($0).map { $0.width > 1 && $0.height > 1 } == true }) {
            try clickSendControl("Details", scrolling: 350)
        }
    }
    func requireSendActions(_ expected: [String]) throws {
        try closeSendDetails()
        let root = nativeRegion("Send and cluster tests")
        let actions = sendActionLabels.filter { label in
            nativeButtons(label, root: root).contains { nativeBounds($0).map { $0.width > 1 && $0.height > 1 } == true }
        }
        require(actions.count <= 2 && actions.sorted() == expected.sorted(), "Unexpected Send header actions: \(actions); expected \(expected)")
    }
    func captureRecoveryActions(_ name: String, retry: Bool = true) throws {
        try requireSendActions(retry ? ["Check cluster status", "Retry cancellation"] : ["Check cluster status"])
        if let bounds = nativeButtons("Check cluster status").compactMap({ nativeBounds($0) }).first {
            scrollMain(Int32((bodyBounds().minY + 32 - bounds.minY).rounded()))
        }
        require(visibleButtons("Check cluster status").count == 1
            && (retry ? visibleButtons("Retry cancellation").count == 1 : nativeButtons("Retry cancellation").isEmpty),
            "Unresolved cancellation lost its supported recovery actions")
        try capture(name, "Designer")
    }
    let normal = try chooseFixture(1)
    // Closing a native select popup can return focus to navigation. Activate the
    // observed Use control instead of assuming Tab left focus on it.
    clickText("Use this cluster", in: normal)
    Thread.sleep(forTimeInterval: 1)
    scrollMain(-400)
    let connected = try capture("cluster-access-compact", "Clusters")
    clickText("Open your agent", in: connected)
    Thread.sleep(forTimeInterval: 0.5)
    let ownedAgent = try capture("agent-cluster-owned", "Designer")
    clickText("Send to a cluster", in: ownedAgent)
    Thread.sleep(forTimeInterval: 1)
    let allowed = try capturePreflight("send-ready", "Ready to send")
    require(allowed.contains { ($0.topCandidates(1).first?.string ?? "").contains("Ready to send") }
        && !nativeButtons("Send and run tests").isEmpty, "Ready destination must expose the Send action")
    func focusSend() throws {
        // Pixel scroll offsets are not stable across responsive reflow. Focus
        // the real entry button, then pointer-click its verified native bounds.
        let matches = nativeElements().filter { element in
            attribute(element, "AXRole") as? String == "AXButton"
                && attribute(element, "AXEnabled") as? Bool == true
                && ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == "Send to a cluster" }
        }
        require(matches.count == 1, "Expected one enabled Send entry")
        let entry = matches[0]
        require(AXUIElementSetAttributeValue(entry, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "Cannot focus Send entry")
        Thread.sleep(forTimeInterval: 0.3)
        guard let focusedBounds = nativeBounds(entry) else { fail("Focused Send entry has no bounds") }
        if !bodyBounds().contains(focusedBounds) {
            scrollMain(Int32((bodyBounds().minY + 32 - focusedBounds.minY).rounded()))
        }
        guard let bounds = nativeBounds(entry), bodyBounds().contains(bounds) else { fail("Focused Send entry is not visible") }
        mouse(.leftMouseDown, CGPoint(x: bounds.midX, y: bounds.midY))
        mouse(.leftMouseUp, CGPoint(x: bounds.midX, y: bounds.midY))
        Thread.sleep(forTimeInterval: 0.5)
        try closeSendDetails()
        // Observe a real Send control after reflow. Vision can omit the small uppercase section label.
        let actions = ["See in Overview", "Fix in Designer", "Cancel tests", "Send and run tests", "Update agent and run tests", "Run cluster tests again", "Check cluster status", "Reload saved files", "Choose a cluster", "Check again"]
        let root = nativeRegion("Send and cluster tests")
        guard let action = actions.first(where: { nativeButtons($0, root: root).filter { nativeBounds($0).map { $0.width > 1 && $0.height > 1 } == true }.count == 1 }) else { fail("Send panel has no recognized action after reflow") }
        _ = try capturePreflight("send-probe", action)
    }
    func resultSize(_ width: CGFloat, _ height: CGFloat) throws {
        // History proof restarts Bridge; resolve this process's window, not the pre-restart AX handle.
        guard let nativeWindow = nativeElements().first(where: { attribute($0, "AXRole") as? String == "AXWindow" }) else {
            fail("No current native result window")
        }
        let frame = NSWindow.frameRect(forContentRect: CGRect(x: 0, y: 0, width: width, height: height),
            styleMask: [.titled, .closable, .miniaturizable, .resizable])
        var size = frame.size
        require(AXUIElementSetAttributeValue(nativeWindow, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!) == .success, "Cannot resize result window")
        Thread.sleep(forTimeInterval: 0.3)
        require(abs(window()!.1.width - size.width) < 3 && abs(window()!.1.height - size.height) < 3, "Result window size was not applied")
        // Reflow can move the result header above the preserved scroll offset.
        // Reopen the existing panel without starting another Send.
        try focusSend()
    }
    func captureAnswer(_ name: String) throws {
        let seen = try capturePreflight("send-probe", "What SHA-256 checksum does")
        guard let question = seen.first(where: { ($0.topCandidates(1).first?.string ?? "").contains("What SHA-256 checksum does") })
            else { fail("Question heading missing after native search") }
        let y = (1 - question.boundingBox.maxY) * window()!.1.height
        scrollMain(-Int32(max(0, y - 180)))
        // A partial result can reflow while scrolling. Re-anchor before qualifying the frozen answer frame.
        let answer = try capturePreflight(name, "What SHA-256 checksum does")
        let text = answer.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        require(text.contains("What SHA-256 checksum does") && text.contains("Fixture answer:"), "Question and answer are not visible together")
    }
    func captureSendPresentation(_ name: String, actions: [String], earlier: String? = nil, destination: String? = nil) throws {
        for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
            try resultSize(width, height)
            try requireSendActions(actions)
            try capturePreflight("\(name)-header-\(size)", actions[0])
            if let earlier = earlier, let destination = destination {
                try capturePreflight("\(name)-summary-\(size)", "Earlier result on \(earlier)")
                // Destination is a separate current-selection check, not an inference from the earlier result.
                try capturePreflight("\(name)-destination-\(size)", destination)
            }
            try clickSendControl("Details", scrolling: -250)
            try capturePreflight("\(name)-details-\(size)", name == "send-earlier-target" ? "Planned agent" : "Check again")
            require(!nativeButtons("Copy Planned agent").isEmpty, "Resources Details did not open")
            if name == "send-unsupported" { require(nativeButtons("Retry cancellation").isEmpty, "Unsupported retry leaked into Details") }
            try closeSendDetails()
        }
    }
    try resultSize(1280, 800)
    try capturePreflight("send-ready-wide", "Ready to send")
    try resultSize(540, 640)
    try capturePreflight("send-first-minimum", "Send and run tests")
    try clickSendControl("Send and run tests", scrolling: -350)
    Thread.sleep(forTimeInterval: 1)
    try capturePreflight("send-result", "1 of 1 tests passed", requiring: { rows in
        rows.contains { $0.topCandidates(1).first?.string.caseInsensitiveCompare("Passed") == .orderedSame }
    })
    try resultSize(1280, 800)
    try capturePreflight("send-result-wide", "1 of 1 tests passed")
    try clickSendControl("published-checksum", scrolling: -250)
    try captureAnswer("send-answer-wide")
    try resultSize(540, 640)
    try captureAnswer("send-answer-minimum")
    try clickSendControl("published-checksum", scrolling: 250)
    try clickSendControl("Details", scrolling: -350)
    let details = try capturePreflight("send-result-details", "Planned agent")
    let detailsText = details.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
    require(detailsText.contains("Planned agent"), "Short agent identity missing in native Send details")
    require(nativeElements().contains { element in
        attribute(element, "AXRole") as? String == "AXButton"
            && ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == "Copy Planned agent" }
            && nativeBounds(element).map { window()!.1.contains($0) } == true
    }, "Labeled Copy icon missing from native Send details")
    try clickSendControl("Details", scrolling: 350)
    // Observe the passing revision before the result walk changes its test file.
    try resultSize(1220, 900)
    try clickSendControl("See in Overview", scrolling: -350)
    Thread.sleep(forTimeInterval: 1)
    let passedUID = try recordShot("overview-fixture-versions-passed-expanded", "Passed")
    clickTab("Designer", [])
    try resultSize(540, 640)
    // Read side effects only from the loopback fixture after real native IPC/HTTP.
    func assertDeployment(_ uid: String, generation: Int, tasks: Int) throws {
        let state = try fixtureControl(["action": "snapshot", "place": "fixture-cluster"])
        let agents = state["agents"] as! [[String: Any]]
        require(agents.count == 1 && agents[0]["uid"] as? String == uid
            && agents[0]["generation"] as? Int == generation && state["tasks"] as? Int == tasks,
            "Repeated Send changed identity, revision or Task count: \(state)")
    }
    try assertDeployment(passedUID, generation: 1, tasks: 1)
    try capturePreflight("send-unchanged", "Run cluster tests again")
    try requireSendActions(["See in Overview", "Run cluster tests again"])
    try clickSendControl("Run cluster tests again", scrolling: -350)
    try capturePreflight("send-unchanged-result", "1 of 1 tests passed")
    try assertDeployment(passedUID, generation: 1, tasks: 2)
    let originalAgent = try Data(contentsOf: ownedFile)
    restoreTemporaryAgent = { try originalAgent.write(to: ownedFile, options: .atomic) }
    defer {
        do { try restoreTemporaryAgent?(); restoreTemporaryAgent = nil }
        catch { fail("Cannot restore original agent file: \(error)") }
    }
    func changeAgent(_ note: String) throws {
        var edited = originalAgent
        edited.append(Data("\n\(note)\n".utf8))
        try edited.write(to: ownedFile, options: .atomic)
        try clickSendControl("Check again", scrolling: -250)
        try clickSendControl("Reload saved files", scrolling: -250)
        try focusSend()
    }
    try changeAgent("Keep answers concise.")
    try capturePreflight("send-update", "Update agent and run tests")
    try clickSendControl("Update agent and run tests", scrolling: -350)
    try capturePreflight("send-update-result", "1 of 1 tests passed")
    try assertDeployment(passedUID, generation: 2, tasks: 3)
    // Older Orka refuses the distinct conditional route. Only an explicit
    // recovery creates the second identity needed by the Overview evidence.
    try changeAgent("Keep answers concise and quote the source.")
    try fixtureControl(["action": "update-mode", "place": "fixture-cluster", "mode": "old-404"])
    try clickSendControl("Update agent and run tests", scrolling: -350)
    try capturePreflight("send-older-orka", "This Orka can't safely update this agent")
    try resultSize(1280, 800)
    try capturePreflight("send-older-orka-wide", "This Orka can't safely update this agent")
    try resultSize(540, 640)
    try focusSend()
    try clickSendControl("Details", scrolling: -250)
    try capturePreflight("send-older-orka-action", "Send as a separate agent")
    try assertDeployment(passedUID, generation: 2, tasks: 3)
    try clickSendControl("Send as a separate agent", scrolling: -250)
    try capturePreflight("send-separate-result", "1 of 1 tests passed")
    let separateState = try fixtureControl(["action": "snapshot", "place": "fixture-cluster"])
    let separateAgents = separateState["agents"] as! [[String: Any]]
    require(separateAgents.count == 2 && Set(separateAgents.compactMap { $0["uid"] as? String }).count == 2
        && separateAgents.contains { $0["uid"] as? String == passedUID && $0["generation"] as? Int == 2 }
        && separateState["tasks"] as? Int == 4, "Separate recovery did not retain the old deployment: \(separateState)")
    try fixtureControl(["action": "update-mode", "place": "fixture-cluster", "mode": "supported"])
    // Add a distinct failing test to the CI-owned saved file. Reload through the
    // existing preflight flow; the earlier pass must turn stale, not stay green.
    let testsURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Orka/agents/web-reader/web-reader.agent.test.md")
    let originalTests = try Data(contentsOf: testsURL)
    restoreTemporaryTests = { try originalTests.write(to: testsURL, options: .atomic) }
    defer {
        do { try restoreTemporaryTests?(); restoreTemporaryTests = nil }
        catch { fail("Cannot restore original test file: \(error)") }
    }
    var mixedTests = originalTests
    mixedTests.append(Data("\n```yaml agent-test\nid: has-a-date\ntitle: Finds the publication date\ninput: Which date is shown on the page?\nassertions:\n  - id: date\n    type: contains\n    value: '2099-12-31'\n```\n".utf8))
    try mixedTests.write(to: testsURL, options: .atomic)
    try clickSendControl("Check again", scrolling: -250)
    try focusSend()
    try capturePreflight("send-disk-stale", "Earlier files")
    try clickSendControl("Reload saved files", scrolling: -250)
    try focusSend()
    try capturePreflight("send-stale", "Earlier files")
    try resultSize(1280, 800)
    try capturePreflight("send-stale-wide", "Earlier files")
    try clickSendControl("Run cluster tests again", scrolling: 350)
    Thread.sleep(forTimeInterval: 1)
    try capturePreflight("send-mixed-wide", "1 of 2 tests passed")
    // Vision can garble this short Inter sentence at a valid scroll position.
    // Keep the failed test/verdict in the captured frame and require the exact
    // native reason (including its expected value) fully above the footer.
    let failureReason = "The answer did not include “2099-12-31”."
    let visibleFailure: ([VNRecognizedTextObservation]) -> Bool = { seen in
        let root = nativeRegion("Send and cluster tests")
        return seen.contains { ($0.topCandidates(1).first?.string ?? "").contains("Failed") }
            && nativeButtons("has-a-date", root: root).compactMap { nativeBounds($0) }.contains { bodyBounds().contains($0) }
            && nativeTextBounds(failureReason, exact: true, root: root).contains { bodyBounds().contains($0) }
    }
    try capturePreflight("send-failed-wide", "Finds the publication date", anchor: failureReason, requiring: visibleFailure)
    require(nativeStrings().contains { $0.contains("has-a-date") }, "Captured test title must retain its code ID")
    try resultSize(540, 640)
    try capturePreflight("send-failed-minimum", "Finds the publication date", anchor: "has-a-date", requiring: visibleFailure)
    try captureSendPresentation("send-failed", actions: ["Fix in Designer", "Run cluster tests again"])
    // Arm only the loopback fixture: first answer completes, second Task stays
    // live. Hold the cancel reply until its in-flight state has been captured.
    try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": "cancelling"])
    try clickSendControl("Run cluster tests again", scrolling: 350)
    Thread.sleep(forTimeInterval: 0.5)
    try capturePreflight("send-running", "Waiting for answers and evaluating tests")
    try resultSize(1280, 800)
    try capturePreflight("send-running-wide", "Waiting for answers and evaluating tests")
    let partial = try capturePreflight("send-running-partial", "published-checksum")
    require(partial.contains { ($0.topCandidates(1).first?.string ?? "").contains("Passed") }, "Cancellation walk needs a completed test before Cancel tests")
    try clickSendControl("published-checksum", scrolling: -250)
    try captureAnswer("send-running-answer-wide")
    try clickSendControl("published-checksum", scrolling: 250)
    try focusSend()
    try clickSendControl("Cancel tests", scrolling: -350)
    // The verified Cancel control is already in view. The old below-editor offset would overscroll into history.
    // Release the synthetic response as soon as pixels are captured. OCR is
    // deliberately outside the existing five-second fixture hold window.
    let cancellingFrame = try capture("send-cancelling", "Designer") {
        try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": "requested"])
    }
    require(cancellingFrame.contains { ($0.topCandidates(1).first?.string ?? "").contains("Cancelling tests") }, "Native cancellation progress missing from captured frame")
    Thread.sleep(forTimeInterval: 0.5)
    try capturePreflight("send-cancel-requested-wide", "Cancellation requested. Waiting for the cluster.")
    try captureRecoveryActions("send-cancel-requested-actions-wide")
    try resultSize(540, 640)
    let requested = try capturePreflight("send-cancel-requested-minimum", "Cancellation requested. Waiting for the cluster.")
    try captureRecoveryActions("send-cancel-requested-actions")
    clickTab("Clusters", requested); clickTab("Designer", requested)
    try focusSend()
    try capturePreflight("send-page-return", "Cancellation requested. Waiting for the cluster.")
    // Widen the CI-owned native window for the real Overview board. This does
    // not change app state; the earlier minimum Send captures remain 540x640.
    let overviewFrame = NSWindow.frameRect(forContentRect: CGRect(x: 0, y: 0, width: 1220, height: 900),
        styleMask: [.titled, .closable, .miniaturizable, .resizable])
    var overviewSize = overviewFrame.size
    require(AXUIElementSetAttributeValue(nativeWindow, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &overviewSize)!) == .success,
            "Cannot resize native Overview window")
    Thread.sleep(forTimeInterval: 0.5)
    require(abs(window()!.1.width - overviewSize.width) < 3 && abs(window()!.1.height - overviewSize.height) < 3,
            "Native Overview size not verified")
    func visibleTextBounds(_ text: String) -> [CGRect] { nativeTextBounds(text) }
    @discardableResult
    func overviewShot(_ name: String, _ required: [String]) throws -> [VNRecognizedTextObservation] {
        scrollMain(12000)
        for _ in 0..<40 {
            let anchors = visibleTextBounds(required[0])
            if !anchors.isEmpty && required.allSatisfy({ nativeTextIsVisible($0) }) {
                // The required state already fits. Reframing can scroll its recovery note away.
                let seen = try capture(name, "Overview", afterFrame: {
                    require(required.allSatisfy({ nativeTextIsVisible($0) }), "Overview state moved during capture: \(required)")
                })
                print("PASS \(name): visible native state \(required)")
                return seen
            }
            scrollMain(-180)
        }
        let diagnostic = Process()
        diagnostic.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        diagnostic.arguments = ["-x", "-o", "-l", String(window()!.0), "\(output)/\(name)-failed.png"]
        try diagnostic.run(); diagnostic.waitUntilExit()
        for element in nativeElements() {
            guard let bounds = nativeBounds(element), window()!.1.contains(bounds) else { continue }
            let values = ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
            let role = attribute(element, "AXRole") as? String ?? "unknown"
            if !values.isEmpty { print("VISIBLE AX \(role): \(values) \(bounds)") }
        }
        fail("Overview state not visible for \(name): \(required)")
    }
    func recordShot(_ name: String, _ checks: String) throws -> String {
        try overviewShot("overview-probe-collapsed", ["web-reader", checks])
        setOverviewExpanded("web-reader", true)
        let viewport = bodyBounds()
        let places = nativeElements().compactMap { element -> (AXUIElement, CGRect)? in
            guard attribute(element, "AXRole") as? String == "AXGroup",
                ["AXTitle", "AXDescription"].contains(where: { (attribute(element, $0) as? String)?.hasSuffix(", \(checks)") == true }),
                let bounds = nativeBounds(element), viewport.contains(bounds) else { return nil }
            return (element, bounds)
        }
        guard let selected = places.min(by: { $0.1.minY < $1.1.minY }) else { fail("No visible Overview place for \(checks)") }
        let place = selected.0
        func placeButton(_ label: String) -> CGRect? {
            guard let bounds = nativeBounds(place) else { return nil }
            let matches = visibleButtons(label).filter { bounds.contains($0) }
            return matches.count == 1 ? matches[0] : nil
        }
        guard let disclosure = placeButton("Details") else { fail("Details not visible inside the \(checks) place") }
        let point = CGPoint(x: disclosure.midX, y: disclosure.midY)
        mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
        Thread.sleep(forTimeInterval: 0.5)
        // Generated IDs are shortened on screen; exercise Copy instead of requiring a raw UID in the layout.
        try overviewShot("overview-probe-expanded", [checks, "Agent ID"])
        guard let copy = placeButton("Copy Agent ID") else { fail("Agent ID Copy not visible inside the \(checks) place") }
        NSPasteboard.general.clearContents()
        let copyPoint = CGPoint(x: copy.midX, y: copy.midY)
        mouse(.leftMouseDown, copyPoint); mouse(.leftMouseUp, copyPoint)
        Thread.sleep(forTimeInterval: 0.5)
        let uid = NSPasteboard.general.string(forType: .string) ?? ""
        require(uid.hasPrefix("uid-bridge-run-"), "Copy did not retain the full synthetic Agent UID")
        NSPasteboard.general.clearContents()
        try capture(name, "Overview")
        if let close = placeButton("Details") {
            let p = CGPoint(x: close.midX, y: close.midY)
            mouse(.leftMouseDown, p); mouse(.leftMouseUp, p)
        }
        setOverviewExpanded("web-reader", false)
        print("PASS \(name): \(checks), retained \(uid)")
        return uid
    }
    try clickSendControl("See in Overview", scrolling: -350)
    Thread.sleep(forTimeInterval: 1)
    let unknownUID = try recordShot("overview-fixture-versions-unknown-expanded", "Result unknown")
    // The rejected update also supersedes the original agent's earlier pass.
    // Either retained identity can therefore correctly lead the unknown cards;
    // their distinct UIDs and exact create count were verified at separate Send.
    require(separateAgents.contains { $0["uid"] as? String == unknownUID }, "Unknown Overview place did not retain an acknowledged Agent UID")
    setOverviewExpanded("runtime-reader", true)
    try overviewShot("overview-fixture-external-expanded", ["runtime-reader", "Running on fixture-cluster, not saved here"])
    setOverviewExpanded("runtime-reader", false)
    // Explicit retries reuse this attempt's exact Task UID. Neither an HTTP
    // acknowledgement nor a lost response may manufacture a terminal result.
    clickTab("Designer", [])
    try resultSize(1280, 800)
    for (mode, expected) in [("denied", "Cannot cancel with this cluster identity"),
                             ("unconfirmed", "Cancellation not confirmed.")] {
        try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": mode])
        try clickSendControl("Retry cancellation", scrolling: -250)
        Thread.sleep(forTimeInterval: 0.5)
        try capturePreflight("send-cancel-\(mode)", expected)
        try captureRecoveryActions("send-cancel-\(mode)-actions")
    }
    try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": "confirmed"])
    try clickSendControl("Retry cancellation", scrolling: -250)
    Thread.sleep(forTimeInterval: 0.5)
    try capturePreflight("send-cancel-confirmed-wide", "Tests cancelled on the cluster. Completed answers kept.")
    try resultSize(540, 640)
    try capturePreflight("send-cancel-confirmed-minimum", "Tests cancelled on the cluster. Completed answers kept.")
    try clickSendControl("published-checksum", scrolling: -250)
    try captureAnswer("send-cancel-answer")
    try resultSize(1280, 800)
    try captureAnswer("send-cancel-answer-wide")
    // A separate run wins the cancel race with a real answer. The date test
    // fails normally; cancellation must not replace it with a Cancelled result.
    try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": "completed"])
    try focusSend()
    try clickSendControl("Run cluster tests again", scrolling: -350)
    Thread.sleep(forTimeInterval: 0.5)
    try capturePreflight("send-race-running", "Waiting for answers and evaluating tests")
    try focusSend()
    try clickSendControl("Cancel tests", scrolling: -350)
    Thread.sleep(forTimeInterval: 0.5)
    try capturePreflight("send-cancel-completion-race", "Tests finished before cancellation.")
    try focusSend()
    try capturePreflight("send-cancel-race-result", "1 of 2 tests passed")
    try capturePreflight("send-cancel-race-failure", "Finds the publication date", anchor: failureReason, requiring: visibleFailure)
    // Restore and actually Send the common file version on the primary cluster
    // before comparing it with the secondary. Merely restoring local bytes does
    // not undo the live spec updates exercised above.
    try restoreTemporaryTests?(); try restoreTemporaryAgent?()
    restoreTemporaryTests = nil; restoreTemporaryAgent = nil
    try clickSendControl("Check again", scrolling: -250)
    try clickSendControl("Reload saved files", scrolling: -250)
    try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": "completed"])
    try clickSendControl("Update agent and run tests", scrolling: -350)
    try capturePreflight("send-restored-shared-version", "1 of 1 tests passed")
    clickTab("Overview", [])
    // Reopening Overview requests the primary place's historical observation;
    // there is no grouping mode or top-level Refresh control in the list.
    clickTab("Clusters", [])
    clickTab("Overview", [])
    Thread.sleep(forTimeInterval: 1)
    // Confirm another real synthetic installation via the existing Use flow.
    clickTab("Clusters", [])
    // The tab restores the primary's detail. Add a cluster from Places, wide, before choosing.
    scrollMain(1000)
    if !visibleButtons("Back to clusters").isEmpty { clickButton("Back to clusters") }
    clickButton("Add a cluster")
    try capture("cluster-add-wide", "Clusters")
    let secondary = try chooseFixture(6)
    clickText("Use this cluster", in: secondary)
    Thread.sleep(forTimeInterval: 1)
    scrollMain(-400)
    try capture("cluster-access-wide", "Clusters")
    // Connected cluster capture matrix. Keep the existing 540x900 compact walk separate.
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        setContent(width, height)
        scrollMain(12000)
        try capture("cluster-success-\(size)", "Clusters")
        require(nativeTextBounds("fixture-secondary", heading: true).count == 1
            && nativeTextIsVisible("Orka in") && nativeStrings().contains { $0.contains("In use") },
            "Connected cluster must show its named heading, Orka location and header selection")
        require(["Change context", "Check access again", "Reload kubeconfig", "Remove"].allSatisfy { nativeButtons($0).isEmpty },
            "Connected maintenance must start behind Show details")
        scrollMain(-12000)
        try capture("cluster-success-\(size)-end", "Clusters")
        require(visibleButtons("Done").count == 1 && visibleButtons("Open your agent").count == 1,
                "Connected handoff actions must be reachable above the footer")
        clickButton("Show details")
        scrollMain(-12000)
        try capture("cluster-details-\(size)", "Clusters")
        require(nativeTextIsVisible("Orka image") && ["Change context", "Check access again", "Reload kubeconfig", "Remove"].allSatisfy { !visibleButtons($0).isEmpty },
                "Details must reveal the cluster image and reachable maintenance actions")
        clickButton("Hide details")
        clickButton("Done")
        scrollMain(12000)
        try capture("cluster-selected-list-\(size)", "Clusters")
        requirePlaceCards()
        scrollMain(-12000)
        try capture("cluster-selected-list-\(size)-end", "Clusters")
        require(nativeTextIsVisible("fixture-secondary") && nativeTextIsVisible("In use"),
                "Selected cluster row must remain visible at \(size)")
        clickButton("Manage")
    }
    setContent(1280, 800)
    // Done leaves the confirmed destination selected and returns to the list.
    clickButton("Done")
    try capture("places-populated", "Clusters")
    // Use on the non-selected row checks its known context without a chooser.
    clickButton("Use")
    Thread.sleep(forTimeInterval: 1)
    try capture("cluster-connect-known", "Clusters")
    clickButton("Use this cluster")
    clickButton("Done")
    // Reconnect the secondary the same way, keeping the later Send walk on it.
    clickButton("Use")
    Thread.sleep(forTimeInterval: 1)
    clickButton("Use this cluster")
    clickButton("Done")
    clickButton("Manage") // keeps the selected cluster's agent handoff
    Thread.sleep(forTimeInterval: 0.5)
    try capture("cluster-manage", "Clusters")
    setContent(540, 900)
    scrollMain(1000)
    try capture("cluster-manage-compact", "Clusters")
    clickButton("Back to clusters")
    try capture("places-populated-compact", "Clusters")
    setContent(1220, 900)
    clickButton("Manage")
    clickButton("Show details")
    clickButton("Change context")
    try capture("cluster-manage-change", "Clusters")
    clickButton("Back to clusters")
    requirePlaceCards() // includes preserving selection when merely revealing context choice
    clickButton("Manage")
    scrollMain(-400)
    clickText("Open your agent", in: try capture("cluster-probe", "Clusters"))
    let secondAgent = try capture("agent-cluster-owned", "Designer")
    clickText("Send to a cluster", in: secondAgent)
    // The primary's restored-version Send already reloaded these exact files.
    try clickSendControl("Check again", scrolling: -250)
    // The primary's result remains visible as Earlier result; destination readiness is a collapsed disclosure.
    try focusSend()
    try capturePreflight("send-secondary-ready", "Send and run tests")
    try captureSendPresentation("send-earlier-target", actions: ["Send and run tests", "See in Overview"],
        earlier: "fixture-cluster", destination: "fixture-secondary")
    try clickSendControl("Send and run tests", scrolling: -350)
    Thread.sleep(forTimeInterval: 1)
    try capturePreflight("send-secondary-result", "1 of 1 tests passed")
    // Explicitly request a separate identity without changing the common files.
    // The fixture withholds the UID receipt and individual readback, not inventory;
    // automatic reconciliation cannot resolve it or adopt it by its visible name.
    // The earlier passing agent remains a valid cross-cluster association.
    try fixtureControl(["action": "lose-agent-receipt", "place": "fixture-secondary"])
    try clickSendControl("Send as a separate agent", scrolling: 350)
    Thread.sleep(forTimeInterval: 1)
    let noReceipt = try capturePreflight("send-no-agent-receipt", "Result unknown")
    try fixtureControl(["action": "offline", "place": "fixture-secondary", "offline": true])
    clickTab("Overview", noReceipt)
    Thread.sleep(forTimeInterval: 1)
    // Manage already observed the cluster's agents; an unavailable revisit keeps that earlier check.
    try overviewShot("overview-fixture-first-read-unavailable", ["Couldn't check just now.", "Showing what Bridge saw", "runtime-reader"])
    try fixtureControl(["action": "offline", "place": "fixture-secondary", "offline": false])
    clickTab("Clusters", []); clickTab("Overview", [])
    Thread.sleep(forTimeInterval: 1)
    setOverviewExpanded("web-reader", true)
    try overviewShot("overview-fixture-multiple-places-expanded", ["web-reader", "This computer", "fixture-secondary", "Passed"])
    require(!nativeTextIsVisible("Earlier Sends"), "Unobserved Send attempts must stay in Send history, not become Overview places")
    setOverviewExpanded("web-reader", false)
    try fixtureControl(["action": "offline", "place": "fixture-secondary", "offline": true])
    defer { try? fixtureControl(["action": "offline", "place": "fixture-secondary", "offline": false]) }
    clickTab("Clusters", []); clickTab("Overview", [])
    Thread.sleep(forTimeInterval: 1)
    setOverviewExpanded("web-reader", true)
    // Anchor the recovery note's source label, not a distant place mark.
    try overviewShot("overview-fixture-offline-expanded", ["fixture-secondary:", "Couldn't check just now.", "Showing what Bridge saw"])
    try overviewShot("overview-fixture-offline-record-expanded", ["Last Send passed", "Result unknown"])
    setOverviewExpanded("web-reader", false)
    try fixtureControl(["action": "offline", "place": "fixture-secondary", "offline": false])
    // Keep the existing preflight-only cards out of the focused Overview walk.
    // These identities still exercise the same blocked states without a Send.
    clickTab("Clusters", [])
    for (index, name, expected) in [(4, "send-blocked", "Blocked"), (5, "send-cannot-check", "Bridge cannot check")] {
        let selected = try chooseFixture(index)
        clickText("Use this cluster", in: selected)
        Thread.sleep(forTimeInterval: 1)
        scrollMain(-400)
        let connected = try capture("cluster-access", "Clusters")
        clickText("Open your agent", in: connected)
        let agent = try capture("agent-cluster-owned", "Designer")
        clickText("Send to a cluster", in: agent)
        Thread.sleep(forTimeInterval: 1)
        for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
            try resultSize(width, height)
            try capturePreflight("\(name)-\(size)", expected)
            try requireSendActions(["Check again"])
        }
        clickTab("Clusters", [])
    }
    // Unsupported is deliberately the last Send: unfinished work must not block
    // the shared-version update and cross-target evidence above. Reuse the known
    // primary destination and CI-owned two-test file, then restore its exact bytes.
    let cancellationTarget = try chooseFixture(1)
    clickText("Use this cluster", in: cancellationTarget)
    scrollMain(-400)
    clickText("Open your agent", in: try capture("cluster-probe", "Clusters"))
    try clickSendControl("Send to a cluster", scrolling: -350, region: nil)
    restoreTemporaryTests = { try originalTests.write(to: testsURL, options: .atomic) }
    try mixedTests.write(to: testsURL, options: .atomic)
    try clickSendControl("Check again", scrolling: -250)
    try clickSendControl("Reload saved files", scrolling: -250)
    try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": "cancelling"])
    try clickSendControl("Run cluster tests again", scrolling: 350)
    try capturePreflight("send-unsupported-running", "Waiting for answers and evaluating tests")
    try clickSendControl("Cancel tests", scrolling: -350)
    try fixtureControl(["action": "cancel-mode", "place": "fixture-cluster", "mode": "unsupported"])
    try capturePreflight("send-cancel-unsupported", "This Orka cannot cancel tests.")
    try captureSendPresentation("send-unsupported", actions: ["Check cluster status"])
    try captureRecoveryActions("send-cancel-unsupported-actions-minimum", retry: false)
    try clickSendControl("Check cluster status", scrolling: -250)
    try capturePreflight("send-unsupported-after-check", "This Orka cannot cancel tests.")
    try captureRecoveryActions("send-unsupported-after-check-actions", retry: false)
    try restoreTemporaryTests?(); restoreTemporaryTests = nil
    // Reopen without a Send. Cluster metadata and saved Send evidence survive;
    // live handles and cluster authority do not.
    app.terminate()
    for _ in 0..<30 {
        if app.isTerminated { break }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    }
    require(app.isTerminated, "Bridge did not quit before Send history check")
    app = try launch()
    app.activate(options: [])
    // A native frame can appear before the relaunched WebView paints anything.
    // Use the same observed page readiness as the initial launch.
    waitForWorkspaceRenderer()
    let restarted = try capture("cluster-place-reopened", "Clusters")
    // Remembered metadata survives, but neither selection nor live access does.
    let remembered = nativeButtons("Use").count
    require(remembered == 4 && nativeButtons("Manage").isEmpty
        && nativeStrings().contains { $0.contains("fixture-cluster") }
        && nativeStrings().contains { $0.contains("fixture-secondary") }
        && nativeStrings().contains { $0.contains("Checked") }
        && !nativeTextIsVisible("No other clusters added yet"),
        "Restart must retain four checked cluster identities, disconnected")
    // Focus and pointer-click the first retained row. Fresh native discovery, not
    // a persisted target/provider handle, supplies its Use action.
    guard let firstConnect = nativeButtons("Use").first else { fail("Remembered Use missing") }
    require(AXUIElementSetAttributeValue(firstConnect, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success,
            "Cannot focus remembered Connect")
    Thread.sleep(forTimeInterval: 0.3)
    guard let connectBounds = nativeBounds(firstConnect), bodyBounds().contains(connectBounds) else {
        fail("Remembered Connect did not scroll into view")
    }
    let connectPoint = CGPoint(x: connectBounds.midX, y: connectBounds.midY)
    mouse(.leftMouseDown, connectPoint); mouse(.leftMouseUp, connectPoint)
    Thread.sleep(forTimeInterval: 1)
    try capture("cluster-connect-known-restarted", "Clusters")
    clickButton("Use this cluster")
    clickButton("Done")
    clickButton("Manage")
    setContent(1280, 800)
    scrollMain(12000)
    try capture("cluster-manage-restarted-wide", "Clusters")
    setContent(540, 900)
    scrollMain(12000)
    try capture("cluster-manage-restarted-compact", "Clusters")
    clickButton("Show details")
    clickButton("Remove")
    try capture("cluster-remove-confirmation", "Clusters")
    clickButton("Keep cluster")
    require(nativeButtons("Remove cluster").isEmpty, "Cancelling Remove did not keep the cluster")
    clickButton("Remove")
    clickButton("Remove cluster")
    try capture("cluster-removed", "Clusters")
    require(nativeButtons("Use").count == remembered - 1 && nativeButtons("Manage").isEmpty,
            "Remove must forget only the selected cluster and clear its selection")
    app.terminate()
    for _ in 0..<30 {
        if app.isTerminated { break }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    }
    require(app.isTerminated, "Bridge did not quit after forgetting a cluster")
    app = try launch()
    app.activate(options: [])
    waitForWorkspaceRenderer()
    try capture("cluster-removal-reopened", "Clusters")
    require(nativeButtons("Use").count == remembered - 1 && nativeButtons("Manage").isEmpty,
            "Removed cluster returned after restart")
    clickTab("Designer", restarted)
    Thread.sleep(forTimeInterval: 1)
    try clickSendControl("Send to a cluster", scrolling: -350, region: nil)
    clickButton("Past Sends")
    guard let historyHeading = nativeButtons("Past Sends").compactMap({ nativeBounds($0) }).first else {
        fail("Saved Send history was not reachable after restart")
    }
    // The label is now a disclosure control, not a standalone OCR heading.
    // Put it near the top so the newest saved summaries fit in the frame.
    scrollMain(Int32((bodyBounds().minY + 24 - historyHeading.minY).rounded()))
    try capture("send-history-restarted", "Designer")
    // Prove the earlier passing summary remains reachable after restart.
    var savedPass = false
    for _ in 0..<6 {
        let seen = try capture("send-probe", "Designer", searching: true)
        if seen.contains(where: { ($0.topCandidates(1).first?.string ?? "").contains("Saved tests: pass") }) {
            try capture("send-history-passed", "Designer")
            savedPass = true
            break
        }
        scrollMain(-150)
    }
    require(savedPass, "Earlier saved pass was not reachable after restart")
    // Open the disclosure belonging to the visible passing record, not a newer
    // interrupted Send. Keep its saved label in view for the native capture.
    var savedOpened = false
    for _ in 0..<6 {
        let seen = try capture("send-probe", "Designer", searching: true)
        if let passed = seen.first(where: { ($0.topCandidates(1).first?.string ?? "").contains("Saved tests: pass") }),
           let disclosure = seen.filter({
               ($0.topCandidates(1).first?.string ?? "").contains("Show saved details")
                   && $0.boundingBox.maxY < passed.boundingBox.minY
           }).max(by: { $0.boundingBox.maxY < $1.boundingBox.maxY }),
           let label = disclosure.topCandidates(1).first?.string {
            clickText(label, in: [disclosure])
            savedOpened = true
            break
        }
        scrollMain(-100)
    }
    require(savedOpened, "Saved passing Send details were not reachable")
    try clickSendControl("published-checksum", scrolling: -100, region: "Past Sends")
    try resultSize(1280, 800)
    // Anchor the entire saved result before looking for answer pixels, not whichever tail survived reflow.
    guard let provenance = nativeTextBounds("Saved on this computer", visibleOnly: false, exact: true).first else {
        fail("Saved answer provenance has no native bounds")
    }
    scrollMain(Int32((bodyBounds().minY + 64 - provenance.minY).rounded()))
    // The answer-only fallback can scroll the question/provenance out of view after reflow.
    // Keep anchoring the whole saved result until one frame proves all three.
    let savedWide = try capturePreflight("send-history-answer-wide", "Fixture answer:", anchor: "Saved on this computer", region: "Past Sends", requiring: { seen in
        seen.contains { ($0.topCandidates(1).first?.string ?? "").contains("What SHA-256 checksum does") }
            && nativeTextIsVisible("Saved on this computer", exact: true)
    })
    require(savedWide.contains { ($0.topCandidates(1).first?.string ?? "").contains("What SHA-256 checksum does") }
        && nativeTextIsVisible("Saved on this computer", exact: true),
            "Wide saved question, answer and local provenance must remain visible after reopening offline")
    try capture("send-history-answer", "Designer")
}
do {
    // Run after the sample/Send checks: a second web-reader must not make their selectors ambiguous.
    let repo = FileManager.default.temporaryDirectory.appendingPathComponent("bridge-own-repo-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: repo, withIntermediateDirectories: false)
    let selectedRepo = repo.resolvingSymlinksInPath()
    let git = Process() // Fixture setup only; the product must never invoke Git for this storage.
    git.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    git.arguments = ["init", "--quiet", selectedRepo.path]
    try git.run(); git.waitUntilExit()
    require(git.terminationStatus == 0, "Cannot initialize disposable own-repo fixture")
    defer { try? FileManager.default.removeItem(at: selectedRepo) }
    func typeText(_ text: String) {
        for character in text.utf16 {
            var unit = character
            for down in [true, false] {
                let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
                event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
                event.flags = []; event.post(tap: .cghidEventTap)
            }
        }
    }
    setContent(1280, 900)
    clickTab("Designer", [])
    scrollMain(12000) // The shared Send/history stack can leave the editor controls far above the viewport.
    clickDesignerView("Agent")
    clickButton("New agent")
    typeText("web-reader")
    key(48)
    let form = try capture("new-agent-probe", "Designer")
    clickText("Your own repo", in: form)
    require(nativeButtons("Create").count == 1 && attribute(nativeButtons("Create")[0], "AXEnabled") as? Bool == false,
            "Own-repo Create must wait for a native folder selection")
    clickButton("Choose repo")
    var panel: AXUIElement?
    for _ in 0..<50 {
        // rfd sets NSSavePanel.message (not its title), and a parented panel is a sheet.
        let matches = nativeElements().filter { candidate in
            let role = attribute(candidate, "AXRole") as? String
            guard role == "AXSheet" || role == "AXWindow" && attribute(candidate, "AXTitle") as? String != "Bridge" else { return false }
            return nativeElements(candidate).contains { attribute($0, "AXRole") as? String == "AXStaticText"
                && attribute($0, "AXValue") as? String == "Choose your repo" }
        }
        require(matches.count < 2, "Ambiguous native repo chooser")
        panel = matches.first
        if panel != nil { break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    guard let chooser = panel, let chooserBounds = nativeBounds(chooser) else { fail("Native repo folder chooser missing") }
    // Drive the real NSOpenPanel's Go to Folder sheet; never inject picker IPC.
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 5, keyDown: down)!
        event.flags = [.maskCommand, .maskShift]; event.post(tap: .cghidEventTap)
    }
    Thread.sleep(forTimeInterval: 0.3)
    typeText(selectedRepo.path)
    key(36)
    var navigated = false
    for _ in 0..<50 {
        let elements = nativeElements(chooser)
        let goSheetOpen = elements.contains { !CFEqual($0, chooser) && attribute($0, "AXRole") as? String == "AXSheet" }
        if !goSheetOpen && elements.contains(where: { element in
            ["AXValue", "AXTitle"].contains { attribute(element, $0) as? String == selectedRepo.lastPathComponent }
        }) { navigated = true; break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(navigated, "Native folder chooser did not navigate to the disposable repo")
    let shot = Process()
    shot.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    shot.arguments = ["-x", "-R", "\(Int(chooserBounds.minX)),\(Int(chooserBounds.minY)),\(Int(chooserBounds.width)),\(Int(chooserBounds.height))",
                      "\(output)/new-agent-repo-chooser.png"]
    try shot.run(); shot.waitUntilExit()
    require(shot.terminationStatus == 0, "Cannot capture native repo chooser")
    key(36) // Confirm the current folder using the real panel default action.
    let preview = selectedRepo.appendingPathComponent("web-reader").path + "/"
    let copy = "Files save automatically. You manage commits in this repo."
    for _ in 0..<50 {
        if nativeTextIsVisible(preview) && !visibleButtons("Change repo").isEmpty { break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    scrollMain(12000)
    require(nativeTextIsVisible(preview) && nativeTextIsVisible(copy), "Selected repo path or own-repo saving explanation missing")
    try capture("new-agent-repo-selected", "Designer")
    clickButton("Create")
    let folder = selectedRepo.appendingPathComponent("web-reader")
    let agent = folder.appendingPathComponent("web-reader.agent.md")
    let tests = folder.appendingPathComponent("web-reader.agent.test.md")
    for _ in 0..<50 {
        if FileManager.default.fileExists(atPath: tests.path) && nativeButtons("Create").isEmpty && nativeTextIsVisible("Instructions") { break }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(try String(contentsOf: agent, encoding: .utf8).contains("name: \"web-reader\"\n"), "Own-repo web-reader was not created")
    try requireBlankPair(agent, tests, "web-reader")
    require(try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted() == ["web-reader.agent.md", "web-reader.agent.test.md"],
            "Own-repo agent folder must contain only the pair, no nested .git or marker")
    require(try FileManager.default.contentsOfDirectory(atPath: selectedRepo.path).sorted() == [".git", "web-reader"],
            "Creation added unexpected repo-root files or markers")
    scrollMain(12000)
    require(nativeTextIsVisible(copy), "Created own-repo editor lost its storage explanation")
    try capture("new-agent-repo-created", "Designer")
}
requireNoLocalWorkspace()
