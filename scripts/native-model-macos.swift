// Exercise the real compiled frontend in the separate synthetic native fixture.
// This is UI evidence only, not a VM, OS credential-store or inference walk.
import Foundation
import AppKit
import CoreGraphics
import ApplicationServices
import Vision
import ImageIO

// Test scope policy without opening a display or launching the fixture.
func runNativeModelScopeTests(_ manifest: NativeModelCaptureManifest) throws {
    func check(_ value: Bool, _ message: String) throws {
        if !value { throw NativeModelHarnessError("Scope self-test: \(message)") }
    }
    func rejects(_ message: String, _ body: () throws -> Void) throws {
        var rejected = false
        do { try body() } catch { rejected = true }
        try check(rejected, message)
    }
    let normal = ["output", "fixture", "light"]
    let defaultRun = try parseNativeModelArguments(normal)
    try check(defaultRun.scope == .all && defaultRun.scope.startsInstalling, "three arguments retain the full installing journey")
    for (name, model, destinations, installing) in [("all", true, true, true), ("model", true, false, true), ("destinations-workspace", false, true, false)] {
        let parsed = try parseNativeModelArguments(normal + [name])
        try check(parsed.scope.runsModel == model && parsed.scope.runsDestinations == destinations && parsed.scope.startsInstalling == installing, "scope dispatch \(name)")
    }
    try check(try parseNativeModelArguments(["output", "fixture", "dark"]).theme == "dark", "dark theme")
    for bad in [[], ["output"], ["output", "fixture"], ["output", "fixture", "sepia"], normal + ["unknown"], normal + ["all", "extra"], ["--self-test", "extra"]] {
        try rejects("invalid invocation \(bad)") { _ = try parseNativeModelArguments(bad) }
    }
    try manifest.validate()
    try check(manifest.model.count == 208 && manifest.destinationsWorkspace.count == 121, "208/121 including Overview list, long-Chat and upgrade captures")
    for name in ["local-catalog-no-destination-wide", "local-workspace-ready-wide", "local-workspace-stop-failed-details-minimum", "local-designer-waiting-chat-minimum", "local-workspace-image-update-minimum", "local-workspace-image-unsupported-minimum"] {
        try check(manifest.destinationsWorkspace.contains(name), "complete allocated journey includes \(name)")
    }
    for scope in NativeModelScope.allCases {
        var coverage = NativeModelCoverage(expected: Set(manifest.names(for: scope)))
        try coverage.record("local-probe"); try coverage.record("local-probe")
        for name in manifest.names(for: scope) { try coverage.record(name) }
        try coverage.finish()
        try check(coverage.names.count == manifest.names(for: scope).count, "probes do not count as named captures")
    }
    var complete = NativeModelCoverage(expected: ["first", "second"])
    try complete.record("first"); try complete.record("second")
    try complete.finish()
    try rejects("duplicate capture rejected") { try complete.record("first") }
    try rejects("wrong scope capture rejected") { try complete.record("elsewhere") }
    var missing = NativeModelCoverage(expected: ["first", "second"])
    try missing.record("first")
    try rejects("missing capture rejected at completion") { try missing.finish() }
    var duplicate = manifest
    duplicate.model[1] = duplicate.model[0]
    try rejects("duplicate manifest entry rejected") { try duplicate.validate() }
    var overlap = manifest
    overlap.destinationsWorkspace[0] = overlap.model[0]
    try rejects("overlapping manifest groups rejected") { try overlap.validate() }
}

enum NativeModelScope: String, CaseIterable {
    case all, model
    case destinationsWorkspace = "destinations-workspace"
    var runsModel: Bool { self != .destinationsWorkspace }
    var runsDestinations: Bool { self != .model }
    var startsInstalling: Bool { self != .destinationsWorkspace }
}
struct NativeModelInvocation {
    let output: String
    let binary: String
    let theme: String
    let scope: NativeModelScope
}
struct NativeModelHarnessError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}
func parseNativeModelArguments(_ arguments: [String]) throws -> NativeModelInvocation {
    guard arguments.count == 3 || arguments.count == 4,
          ["light", "dark"].contains(arguments[2]),
          let scope = NativeModelScope(rawValue: arguments.count == 4 ? arguments[3] : "all") else {
        throw NativeModelHarnessError("Usage: native-model-macos.swift OUTPUT FIXTURE light|dark [all|model|destinations-workspace], or --self-test")
    }
    return NativeModelInvocation(output: arguments[0], binary: arguments[1], theme: arguments[2], scope: scope)
}
struct NativeModelCaptureManifest: Decodable {
    var model: [String]
    var destinationsWorkspace: [String]
    enum CodingKeys: String, CodingKey {
        case model
        case destinationsWorkspace = "destinations-workspace"
    }
    func names(for scope: NativeModelScope) -> [String] {
        switch scope {
        case .all: return model + destinationsWorkspace
        case .model: return model
        case .destinationsWorkspace: return destinationsWorkspace
        }
    }
    func validate() throws {
        guard model.count == 208, destinationsWorkspace.count == 121,
              Set(model).count == model.count,
              Set(destinationsWorkspace).count == destinationsWorkspace.count,
              Set(model).isDisjoint(with: Set(destinationsWorkspace)),
              !(model + destinationsWorkspace).contains("local-probe") else {
            throw NativeModelHarnessError("Capture manifest must partition 329 unique named captures into 208 model and 121 destination/workspace names")
        }
    }
}
struct NativeModelCoverage {
    let expected: Set<String>
    private(set) var names: [String] = []
    mutating func record(_ name: String) throws {
        if name == "local-probe" { return }
        guard expected.contains(name) else { throw NativeModelHarnessError("Named capture outside selected scope: \(name)") }
        guard !names.contains(name) else { throw NativeModelHarnessError("Duplicate named capture: \(name)") }
        names.append(name)
    }
    func finish() throws {
        guard Set(names) == expected else {
            throw NativeModelHarnessError("Missing named captures: \(expected.subtracting(names).sorted())")
        }
    }
}
func nativeModelStartup() -> (NativeModelInvocation, NativeModelCaptureManifest) {
    do {
        let arguments = Array(CommandLine.arguments.dropFirst())
        let manifestURL = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("native-model-captures.json")
        if arguments == ["--self-test"] {
            let manifest = try JSONDecoder().decode(NativeModelCaptureManifest.self, from: Data(contentsOf: manifestURL))
            try runNativeModelScopeTests(manifest)
            print("PASS native model scope arguments, manifest partition and runtime coverage checker")
            exit(0)
        }
        let invocation = try parseNativeModelArguments(arguments)
        let manifest = try JSONDecoder().decode(NativeModelCaptureManifest.self, from: Data(contentsOf: manifestURL))
        try manifest.validate()
        return (invocation, manifest)
    } catch {
        FileHandle.standardError.write(Data("\(error)\n".utf8))
        exit(2)
    }
}
// All argument/self-test paths finish before the first native global initializer.
let (invocation, captureManifest) = nativeModelStartup()
let scope = invocation.scope
var coverage = NativeModelCoverage(expected: Set(captureManifest.names(for: scope)))

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
let output = invocation.output
let theme = invocation.theme
require(["light", "dark"].contains(theme), "Expected light or dark")
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
let child = Process()
child.executableURL = URL(fileURLWithPath: invocation.binary)
let keyCheckRelease = URL(fileURLWithPath: output).appendingPathComponent("key-check-release-\(UUID().uuidString)")
let setupProgress = URL(fileURLWithPath: output).appendingPathComponent("setup-progress-\(UUID().uuidString)")
let catalogRelease = URL(fileURLWithPath: output).appendingPathComponent("catalog-release-\(UUID().uuidString)")
defer { try? FileManager.default.removeItem(at: catalogRelease) }
// Written only for the fresh-profile first-test outcomes; absent keeps the fixed failed report.
let firstTestOutcome = URL(fileURLWithPath: output).appendingPathComponent("first-test-outcome-\(UUID().uuidString)")
defer { try? FileManager.default.removeItem(at: firstTestOutcome) }
if scope.startsInstalling {
    try "installing".write(to: setupProgress, atomically: true, encoding: .utf8)
}
defer { try? FileManager.default.removeItem(at: setupProgress) }
let chatPhase = URL(fileURLWithPath: output).appendingPathComponent("chat-phase-\(UUID().uuidString)")
try "pending".write(to: chatPhase, atomically: true, encoding: .utf8)
defer { try? FileManager.default.removeItem(at: chatPhase) }
child.environment = ProcessInfo.processInfo.environment.merging(["BRIDGE_MODEL_CHECK_RELEASE": keyCheckRelease.path, "BRIDGE_SETUP_PROGRESS": setupProgress.path, "BRIDGE_FIRST_TEST_OUTCOME": firstTestOutcome.path, "BRIDGE_CHAT_PHASE": chatPhase.path, "BRIDGE_CATALOG_RELEASE": catalogRelease.path]) { _, fixture in fixture }
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
    if name.hasPrefix("local-") || name.hasPrefix("chat-") || name.hasPrefix("cluster-") {
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
        // Upgrade expectations are paragraph substrings, as in the OCR check.
        // Match unchanged text in a fully visible native node, never an OCR spelling alias.
        let upgradeText = !found && name.hasPrefix("local-upgrade-") && nativeChatTextIsVisible(value, aboveFooter: true)
        let chatText = !found && (name.hasPrefix("chat-") || name.hasPrefix("cluster-") || name.hasPrefix("local-shell-chat-")) && nativeChatTextIsVisible(value)
        let workspaceControl = !found && name.hasPrefix("local-workspace-") && nativeWorkspaceButtonIsVisible(value)
        let overviewControl = !found && name.hasPrefix("local-") && name.contains("overview") && nativeWorkspaceButtonIsVisible(value)
        let overviewHeading = !found && value == "web-reader" && name.hasPrefix("local-overview-")
            && visible.contains("web-xeader") && nativeOverviewHeadingIsVisible(value)
        require(found || nativeText || upgradeText || chatText || workspaceControl || overviewControl || overviewHeading, "Missing \(value) in \(name): \(text)")
    }
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: file) as CFURL, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
        let pixel = image.cropping(to: CGRect(x: CGFloat(image.width - 20), y: CGFloat(image.height - 20), width: 1, height: 1)),
        let space = CGColorSpace(name: CGColorSpace.sRGB) else { fatalError("Cannot decode native theme") }
    var rgba = [UInt8](repeating: 0, count: 4)
    let sampled = rgba.withUnsafeMutableBytes { bytes in
        guard let context = CGContext(data: bytes.baseAddress, width: 1, height: 1,
            bitsPerComponent: 8, bytesPerRow: 4, space: space,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
            else { return false }
        context.draw(pixel, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        return true
    }
    require(sampled, "Cannot sample native theme in sRGB")
    let pickerOpen = enabledButton("Close picker") != nil
    var expected: [CGFloat] = theme == "dark" ? [9, 9, 10] : [255, 255, 255]
    if pickerOpen {
        expected = theme == "dark" ? expected.map { $0 * 0.45 }
            : zip(expected, [CGFloat](repeating: 11, count: 3)).map { $0 * 0.82 + $1 * 0.18 }
    }
    let actual = rgba.prefix(3).map { CGFloat($0) }
    let tolerance: CGFloat = pickerOpen ? 3 : 8
    require(zip(actual, expected).allSatisfy { abs($0 - $1) < tolerance }, "Native fixture does not use Orka paper: \(actual)")
    // Setup frames 1c, 1d (cropped) and 1e exist only in light; dark captures use them as layout references.
    // Frames 1g (AI connection failed) and 1h (AI reset) are approved in both themes.
    let frame: (file: String, label: String)? =
        name.hasPrefix("chat-") ? ("designer-reference-\(theme)", "Designer design system (no dedicated Chat frame)")
        : name.hasPrefix("cluster-") ? ("cluster-reference-light", "Approved cluster cards (wide reference)")
        : name.hasPrefix("local-workspace-") || name == "local-stopped" ? ("workspace-reference-\(theme)", "Approved workspace frame (wide reference)")
        : ["local-result", "local-answer", "local-story", "local-running", "local-stopping", "local-shell", "local-catalog-no-destination"].contains(where: { name.hasPrefix($0) }) ? ("designer-reference-\(theme)", "Approved Designer frame (wide reference)")
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
    try coverage.record(name)
    print("PASS \(name): real native frontend, synthetic IPC only")
    return rows
}
func enabledButton(_ label: String) -> AXUIElement? {
    let buttons = nativeElements().filter { element in
        guard let role = attribute(element, "AXRole") as? String else { return false }
        // Keep ordinary controls distinct from the context's native popup trigger.
        return (["AXButton", "AXCheckBox"].contains(role) || ["kubectl context", "ServiceAccount"].contains(label) && role == "AXPopUpButton")
            && attribute(element, "AXEnabled") as? Bool == true
            && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == label })
    }
    return buttons.count == 1 ? buttons[0] : nil
}
func setOverviewExpanded(_ title: String, _ expanded: Bool) {
    let label = "Show places for \(title)"
    let buttons = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXButton"
            && ["AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == label }
    }
    require(buttons.count == 1, "Cannot identify Overview disclosure \(label)")
    if attribute(buttons[0], "AXExpanded") as? Bool == expanded { return }
    let top = enabledButton("Overview").flatMap(nativeBounds)?.maxY ?? window()!.1.minY
    for _ in 0..<12 {
        if let bounds = enabledButton(label).flatMap(nativeBounds) {
            if bounds.minY >= top + 8 && bounds.maxY <= window()!.1.maxY - 32 {
                click(label, []); return
            }
            scroll(bounds.minY < top + 8 ? 150 : -150)
        } else { scroll(-150) }
        Thread.sleep(forTimeInterval: 0.1)
    }
    require(false, "Cannot bring Overview disclosure into view: \(label)")
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
// Keep journey bodies unchanged; only execution is partitioned.
var rows: [VNRecognizedTextObservation] = []
if scope.runsModel {
// Exercise the four-step list with only synthetic IPC, at the actual minimum and wide sizes.
waitForButton("Change")
for (phase, expected) in [("installing", "Downloading, 1.1 of 2.1 GB"), ("starting", "45 seconds elapsed")] {
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
    if phase == "installing" { try setupFitCapture("download", ["Meet your first agent", "Run your first agent"], "Read its instructions") }
}
localSize(1280, 800)
scroll(4000)
// Default shows one Cancel (pauses setup); queueing the AI goes through its own inline Change first.
click("Change", [])
click("Connect the AI", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("connects when", -200)
    _ = try capture("local-setup-queued-\(size)", ["Queued", "Change"])
    require(enabledButton("Cancel queued connection") == nil, "A queued connection must not offer its own Cancel")
}
localSize(1280, 800); scroll(4000)
// Changing the provider choice while queued clears the queue; restore Copilot before continuing.
click("Change", [])
chooseConnection("Your own API key")
require(!nativeTextExists("Queued"), "Changing the radio choice must clear the queue")
chooseConnection("GitHub Copilot")
click("Change", [])
click("Show details", [])
_ = try capture("local-setup-starting-details", ["45 seconds elapsed", "Starting the workspace"])
try FileManager.default.removeItem(at: setupProgress)
Thread.sleep(forTimeInterval: 0.6)
click("Clusters", [])
waitForButton("Connect the AI")
// A running VM is not completed setup: its local row offers the current AI action.
rows = try capture("local-places-wide", ["Choose where agents run", "This computer", "Connect the AI"])
require(enabledButton("Connect the AI") != nil && enabledButton("Manage") == nil && enabledButton("Set up") == nil,
        "VM-ready local card must offer Connect the AI, not Manage")
click("Connect the AI", rows)
rows = try capture("before-connect", ["Connect the AI"])
// The tab subtitle is visible in the capture but isn't a separate AXStaticText value.
require(rows.contains { $0.boundingBox.minX < 0.3 && $0.boundingBox.maxY > 0.85 && $0.topCandidates(1).first?.string == "4 saved agents" }, "Hidden Overview must report the saved-file count in the header")
click("Overview", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    let longContext = "context-" + String(repeating: "unbrokensegment", count: 7)
    guard let contextBounds = enabledButton(longContext).flatMap(nativeBounds) else { fatalError("Missing long Overview context filter") }
    require(contextBounds.minX >= window()!.1.minX && contextBounds.maxX <= window()!.1.maxX,
            "Long Overview context filter must fit the native window")
    require(!anyButton("By cluster") && !anyButton("By agent") && !anyButton("Refresh"),
            "Overview list must not retain grouping or top-level Refresh controls")
    require(nativeResultTextIsVisible("Connect the AI in Clusters to chat or run tests on this computer."),
            "Disconnected Overview must explain the local AI gate")
    _ = try capture("local-overview-ai-disconnected-collapsed-\(size)", ["Your agents"])
    setOverviewExpanded("web-reader", true)
    require(anyButton("Run tests web-reader") && enabledButton("Run tests web-reader") == nil && enabledButton("Open web-reader") != nil,
            "Without connected AI, Overview must disable local Run but preserve Open")
    _ = try capture("local-overview-ai-disconnected-expanded-\(size)", ["Your agents", "web-reader", "This computer", "Run tests"])
    setOverviewExpanded("web-reader", false)
}
localSize(1280, 800); scroll(4000)
click("Clusters", []); waitForButton("Connect the AI")
localSize(540, 640)
scroll(4000)
_ = try capture("local-setup-ai-minimum-top", ["This computer", "Get your workspace ready"])
_ = try seek("Connect the AI", -200)
_ = try capture("local-setup-ai-minimum", ["GitHub Copilot", "Connect the AI"])
try setupFitCapture("ai", ["Workspace", "GitHub Copilot"], "Connect the AI")
// Earlier-installation record (frames 1g/1h): synthetic recovery IDs, no store, file or VM.
try "model-mismatch".write(to: setupProgress, atomically: true, encoding: .utf8)
press("Connect the AI")
waitForButton("Reset AI connection")
require(nativeTextExists("The AI couldn't connect") && nativeTextExists("Your agents are safe."), "Mismatch must name the AI failure and the safe workspace")
require(!["Remove sign-in", "Check again", "Connect the AI"].contains(where: anyButton), "Mismatch offers only reset recovery")
try resetCapture("mismatch", ["earlier setup", "Use another provider", "Copy details for help"], anchor: "Reset AI connection")
press("Clusters")
waitForButton("Reset AI connection")
try resetCapture("place", ["This computer", "The AI needs to be set up again", "earlier setup"], anchor: "Reset AI connection")
press("View workspace")
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
rows = try capture("local-completed", ["This computer", "Workspace ready", "Run your first agent"])
require(!rows.contains { ($0.topCandidates(1).first?.string ?? "").range(of: "[Ss]tep [0-9]+ of [45]", options: .regularExpression) != nil },
        "Completed local page must not keep a setup counter")
// Connected now only exposes Change; it opens a panel before the existing removal confirmation.
click("Change", rows)
for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
    localSize(width, height); scroll(size == "minimum" ? -4000 : 4000)
    rows = try capture("local-change-panel-\(size)", ["Change the AI", "Remove saved sign-in", "Done"])
}
localSize(1280, 800); scroll(4000)
click("Remove saved sign-in", rows)
for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
    localSize(width, height); scroll(size == "minimum" ? -4000 : 4000)
    rows = try capture("local-remove-confirmation-\(size)", ["saved sign-in?", "Cancel", "Remove sign-in"])
}
localSize(1280, 800); scroll(4000)
click("Cancel", rows)
click("Done", rows)
rows = try capture("connected", ["Connected", "Change"])
require(nativeChatTextIsVisible("synthetic-user") && nativeChatTextIsVisible("gpt-5.6-terra"), "Connected Copilot must show the verified account and fixed model")
localSize(540, 640); scroll(4000)
_ = try seek("Saved GitHub sign-in", -200)
_ = try capture("local-setup-connected-minimum", ["GitHub Copilot", "gpt-5.6-terra"])
localSize(1280, 800); scroll(4000)
click("Change", rows)
click("Remove saved sign-in", rows)
rows = try capture("local-remove-confirmed", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Remove sign-in", rows)
rows = try capture("local-remove-failed", ["Couldn't remove the saved sign-in", "Retry removal", "Copy details for help"])
require(nativeTextExists("The saved sign-in couldn't be removed") && !anyButton("Remove saved sign-in"), "Removal failure needs its own heading and no Remove")
require(enabledButton("Connect the AI") == nil, "Failed cleanup must offer removal retry, not connection")
let cleanupCases: [(String?, String, String)] = [(nil, "disconnectRetainedModel", "refused"), ("cleanup-status", "modelStatus", "unsupported"), ("cleanup-preconditions", "modelStatusPreconditions", "refused")]
for (index, item) in cleanupCases.enumerated() {
    if let phase = item.0 {
        try phase.write(to: setupProgress, atomically: true, encoding: .utf8)
        press("Retry removal")
        rows = try capture("local-remove-diagnostic-\(index)", ["Couldn't remove the saved sign-in", "Retry removal", "Copy details for help"])
    }
    NSPasteboard.general.clearContents()
    press("Copy details for help")
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    let cleanupDetails = NSPasteboard.general.string(forType: .string) ?? ""
    require(cleanupDetails.contains("Cleanup operation: \(item.1)") && cleanupDetails.contains("Cleanup class: \(item.2)"), "Removal Copy details must retain each finite cleanup operation/class")
    require(!cleanupDetails.contains("synthetic-private-token"), "Removal Copy details must not include raw errors")
}
try FileManager.default.removeItem(at: setupProgress)
press("Retry removal")
rows = try capture("removed", ["Saved sign-in removed.", "Connect the AI"])
click("Connect the AI", rows)
rows = try capture("reconnected", ["Connected", "Change"])
click("Overview", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try capture("local-overview-ai-connected-collapsed-\(size)", ["Your agents", "web-reader"])
    setOverviewExpanded("web-reader", true)
    require(enabledButton("Run tests web-reader") != nil && !nativeTextExists("Connect the AI in Clusters to chat or run tests on this computer."),
            "Connected AI must enable the failed agent's local Run action without the connection reason")
    _ = try capture("local-overview-ai-connected-expanded-\(size)", ["Your agents", "This computer", "Run tests"])
    setOverviewExpanded("web-reader", false)
}
localSize(1280, 800); scroll(4000)
click("Clusters", [])
// Returning workspace observations follow the original baseline coverage below.
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
            if strings.contains(where: { $0.contains("These checks do not change") || $0.contains("This computer has") || $0.contains("Virtualization:") || $0.contains("Bridge couldn't check") }) { checksSettled = true; break }
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
            let unchecked = state == "mixed" ? "virtualization" : "virtualization or memory"
            require(strings.contains { $0.contains("Bridge couldn't check \(unchecked) yet. It checks again when the workspace starts.") },
                    "Unknown checks need one quiet startup explanation")
        }
        if ["memory", "mixed"].contains(state) {
            require(strings.contains { $0.contains("This computer has 2.1 GB of memory; Bridge needs 4.3 GB.") }, "Insufficient RAM must show its native amounts")
            require(enabledButton("Check again") == nil && enabledButton("Copy details for help") == nil, "RAM failure needs only the cluster alternative")
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
_ = try seek("Tests on this computer", -200)
let firstResult = try capture("local-first-test-result", ["Tests on this computer", "1 of 2 passed"])
require(!firstResult.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ").contains("Your agent selection could not be saved"), "Fixture rejected its owned agent selection")
require(!exactTextIsVisible("Saved"), "Untouched agent must not claim a save acknowledgement")
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
}
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
if scope.runsModel {
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
        try setupFitCapture("result", ["Your first agent passed its test", "Open web-reader"], "Open web-reader")
        require(enabledButton("Stop the workspace") == nil, "The first-pass story must not offer Stop")
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
}
// Compare a real accessibility control before/after scrolling, not an image hash or a viewport guess.
func setupFitCapture(_ name: String, _ expected: [String], _ anchor: String) throws {
    localSize(1028, 729); scroll(4000)
    guard let button = enabledButton(anchor), let before = nativeBounds(button) else { require(false, "Missing setup-fit anchor: \(anchor)"); return }
    _ = try capture("local-setup-fit-\(name)", expected)
    scroll(-4000)
    guard let after = nativeBounds(button) else { require(false, "Lost setup-fit anchor: \(anchor)"); return }
    require(abs(before.minY - after.minY) <= 1, "Setup must not scroll at 1028×729: \(name)")
    localSize(1280, 800); scroll(4000)
}
func scroll(_ pixels: Int32) {
    let rect = window()!.1
    let event = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: pixels, wheel2: 0, wheel3: 0)
    event?.location = CGPoint(x: rect.maxX - 20, y: rect.midY)
    event?.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.3)
}
func seek(_ label: String, _ pixels: Int32, diagnosticPrefix: String? = nil) throws -> [VNRecognizedTextObservation] {
    for attempt in 0..<6 {
        let seen = try capture("local-probe", [])
        // Preserve the first viewport and every recovery step, even when seek succeeds.
        // Copies are diagnostics, not new named capture cases.
        if let prefix = diagnosticPrefix {
            let frame = window()!.1
            let stem = "\(output)/\(prefix)-attempt-\(attempt)"
            for ext in ["png", "txt"] {
                try FileManager.default.copyItem(atPath: "\(output)/local-probe.\(ext)", toPath: "\(stem).\(ext)")
            }
            let trace: [String: Any] = ["label": label, "pixels": pixels, "attempt": attempt, "width": frame.width, "height": frame.height]
            try JSONSerialization.data(withJSONObject: trace, options: [.sortedKeys]).write(to: URL(fileURLWithPath: "\(stem).json"))
        }
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
    guard let frame = window()?.1 else { fatalError("Cannot find the captured fixture window") }
    // AXWindows also includes transient auxiliary windows; never resize the first one blindly.
    let matches = (windows as! [AXUIElement]).filter { element in
        guard let bounds = nativeBounds(element) else { return false }
        return abs(bounds.minX - frame.minX) < 3 && abs(bounds.minY - frame.minY) < 3
            && abs(bounds.width - frame.width) < 3 && abs(bounds.height - frame.height) < 3
    }
    require(matches.count == 1, "Expected one native window matching the captured frame, found \(matches.count)")
    let native = matches[0]
    var origin = CGPoint(x: 0, y: 40)
    require(AXUIElementSetAttributeValue(native, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &origin)!) == .success, "Cannot position fixture window")
    var size = NSWindow.frameRect(forContentRect: CGRect(x: 0, y: 0, width: width, height: height), styleMask: [.titled, .closable, .miniaturizable, .resizable]).size
    let before = nativeBounds(native)
    let result = AXUIElementSetAttributeValue(native, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!)
    require(result == .success, "Cannot resize fixture window: AX=\(result.rawValue), title=\(attribute(native, "AXTitle") as? String ?? "unnamed"), windows=\((windows as! [AXUIElement]).count), before=\(String(describing: before)), requested=\(size)")
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
if scope.runsModel {
// Change opens a panel with a quiet Remove action, then the existing removal confirmation.
click("Change", [])
click("Remove saved sign-in", [])
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
waitForNativeText("Anthropic didn't accept this key")
require(anyButton("Check again") && enabledButton("Check again") == nil, "An empty replacement key must leave Check again disabled")
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
click("Remove saved key", [])
click("Remove key", [])
try keyCapture("change-failed", ["Key removal has not been confirmed", "Retry removal"])
require(nativeTextExists("Couldn't remove the saved key") && !anyButton("Remove saved key"), "Key removal failure needs its own heading and no Remove")
require(enabledButton("Anthropic") == nil, "Change must not permit replacement before cleanup")
click("Retry removal", [])
try keyCapture("changed", ["API key", "Key removed."], compact: true)
fillMessage("sk-ant-synthetic-only-not-a-real-key", secure: true)
_ = try seek("Connect the AI", -200)
click("Connect the AI", [])
waitForButton("Change")
_ = try seek("Change", -200)
click("Change", [])
click("Remove saved key", [])
try keyCapture("remove-confirmation", ["saved key?", "Cancel", "Remove key"], compact: true)
click("Remove key", [])
try keyCapture("removed", ["API key", "Key removed."])
chooseConnection("GitHub Copilot")
_ = try seek("Connect the AI", -200)
click("Connect the AI", [])
// Restore Copilot before the original result, editor and Chat captures.
scroll(4000)
rows = try seek("Read its instructions", -200)
click("Read its instructions", rows)
rows = try seek("Tests on this computer", -250)
_ = try capture("local-result-wide", ["Tests on this computer", "1 of 2 passed"])
_ = try seek("The answer did not include", -150)
_ = try capture("local-answer-wide", ["Which date is shown", "No date was given", "2099-12-31"])
localSize(540, 900)
scroll(4000)
_ = try seek("Tests on this computer", -250)
_ = try capture("local-result", ["Tests on this computer", "1 of 2 passed"])
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
_ = try seek("Use", -150)
click("Use", [])
_ = try seek("Use this cluster", -150)
click("Use this cluster", [])
_ = try seek("Open your agent", -150)
click("Open your agent", [])
// A remote place blocks Chat: no Ready claim and a disabled New conversation, at both sizes.
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    waitForButton("Chat"); click("Chat", [])
    waitForChatText("Chat on this computer")
    require(!nativeChatTextIsVisible("Ready for your message."), "Blocked Chat must not say Ready")
    require(anyButton("New conversation") && enabledButton("New conversation") == nil, "Blocked Chat must disable New conversation")
    _ = try capture("local-shell-chat-blocked-\(size)", ["Chat on this computer"])
    _ = try seek("Chat is unavailable", -150)
    scroll(-100) // Keep the status clear of the fixed footer.
    _ = try capture("local-shell-chat-blocked-status-\(size)", ["Chat is unavailable."])
    scroll(4000)
    click("Agent", [])
}
}
func focusedButtonTitle() -> String? {
    var focused: CFTypeRef?
    let app = AXUIElementCreateApplication(child.processIdentifier)
    guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
        let focused else { return nil }
    let element = focused as! AXUIElement
    return attribute(element, "AXTitle") as? String ?? attribute(element, "AXDescription") as? String
}
if scope.runsModel {
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
}
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
func nativeOverviewHeadingIsVisible(_ expected: String) -> Bool {
    guard let headerBottom = enabledButton("Overview").flatMap(nativeBounds)?.maxY else { return false }
    return nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXHeading",
            ["AXValue", "AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == expected }),
            let bounds = nativeBounds(element) else { return false }
        return bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)
            && bounds.minY >= headerBottom + 8 && bounds.maxY <= window()!.1.maxY - 32
    }
}
func nativeChatTextIsVisible(_ expected: String, aboveFooter: Bool = false) -> Bool {
    nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXStaticText",
            let text = attribute(element, "AXValue") as? String,
            text.contains(expected), let bounds = nativeBounds(element) else { return false }
        // WebKit exposes clipped or screen-reader-only text with zero/one-pixel bounds.
        return bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)
            && (!aboveFooter || bounds.maxY <= window()!.1.maxY - 32)
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
func nativeHeadings(_ expected: String) -> [AXUIElement] {
    nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXHeading"
            && ["AXValue", "AXTitle", "AXDescription"].contains { attribute(element, $0) as? String == expected }
    }
}
func nativeHeadingExists(_ expected: String) -> Bool { !nativeHeadings(expected).isEmpty }
func anchorChatHeading(_ expected: String) {
    // Resizing can leave Chat following the composer until its next status poll.
    // A real scroll away from the end disables follow before measuring the heading.
    scroll(4000)
    let headings = nativeHeadings(expected)
    guard headings.count == 1, let bounds = nativeBounds(headings[0]),
        let header = enabledButton("Designer").flatMap(nativeBounds) else {
        require(false, "Cannot anchor native Chat heading: \(expected)"); return
    }
    // Clear fixed navigation and the 48px composer-follow threshold before freezing the frame.
    let delta = header.maxY + 64 - bounds.minY
    if abs(delta) > 2 { scroll(Int32(delta.rounded())) }
    guard let aligned = nativeHeadings(expected).first.flatMap(nativeBounds) else {
        require(false, "Lost native Chat heading after scrolling: \(expected)"); return
    }
    require(window()!.1.contains(aligned) && aligned.minY >= header.maxY + 8,
        "Native Chat heading is clipped after scrolling: \(expected), bounds=\(aligned), header=\(header)")
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
        var contentVisible = false
        for _ in 0..<10 {
            let anchorVisible = enabledButton(anchor).map { reachable(nativeBounds($0)) } ?? nativeChatTextIsVisible(anchor, aboveFooter: true)
            // Wrapped secondary actions may sit below the primary at the minimum size.
            let actionsVisible = expected.compactMap(enabledButton).allSatisfy { reachable(nativeBounds($0)) }
            if anchorVisible && actionsVisible {
                let probe = try capture("local-probe", [])
                let text = probe.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
                // Copy notices and disclosure text can also wrap below the last action.
                if expected.allSatisfy({ text.contains($0) || nativeChatTextIsVisible($0, aboveFooter: true) }) { contentVisible = true; break }
            }
            scroll(-200)
        }
        require(contentVisible, "Reset capture content must be visible above the footer: \(state)-\(size)")
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
func fillMessage(_ text: String, secure: Bool = false, fieldLabel: String? = nil) {
    let label = fieldLabel ?? (secure ? "API key" : "Message")
    let fields = nativeElements().filter { element in
        (fieldLabel != nil ? ["AXTextField", "AXSearchField"] : secure ? ["AXTextField", "AXSecureTextField"] : ["AXTextArea"]).contains(attribute(element, "AXRole") as? String ?? "")
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
    for (lineIndex, line) in text.components(separatedBy: "\n").enumerated() {
        // A Unicode payload beginning with a newline can drop its following text.
        // Use the real newline gesture, never an unmodified Enter that submits Chat.
        if lineIndex > 0 {
            for down in [true, false] {
                let event = CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: down)!
                event.flags = .maskShift; event.post(tap: .cghidEventTap)
            }
        }
        let characters = Array(line.utf16)
        for start in stride(from: 0, to: characters.count, by: 16) {
            let units = Array(characters[start..<min(start + 16, characters.count)])
            for down in [true, false] {
                let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
                units.withUnsafeBufferPointer { event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress!) }
                event.flags = []; event.post(tap: .cghidEventTap)
            }
            if text.utf16.count > 256 { Thread.sleep(forTimeInterval: 0.08) }
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
let replyReason = "Conversation replies need an authenticated gateway conversation, which local tests and Chat do not have."
func typeUnicode(_ text: String) {
    let characters = Array(text.utf16)
    for start in stride(from: 0, to: characters.count, by: 16) {
        let units = Array(characters[start..<min(start + 16, characters.count)])
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
            units.withUnsafeBufferPointer { event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress!) }
            event.flags = []; event.post(tap: .cghidEventTap)
        }
    }
    Thread.sleep(forTimeInterval: 0.3)
}
if scope.runsModel {
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
    // Filter after the approval frame so the reply row and any reason must fit at every size, minimum included.
    require(AXUIElementSetAttributeValue(search[0], kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "Cannot focus picker search")
    typeUnicode("reply")
    let replyButtons = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXButton"
            && ["AXTitle", "AXDescription"].contains { (attribute(element, $0) as? String)?.contains("reply_in_conversation") == true }
    }
    require(replyButtons.count == 1, "Search must leave exactly the reply tool row")
    require(attribute(replyButtons[0], "AXEnabled") as? Bool == false, "Reply tool must stay disabled locally")
    // Require the whole reason in the captured window, not only accessibility-node geometry.
    _ = try capture("local-reply-tool-\(size)", ["Find a tool", "Reply in a conversation", replyReason])
    guard let close = enabledButton("Close picker"), let bounds = nativeBounds(close) else {
        fatalError("Labeled picker X missing")
    }
    require(abs(bounds.width - 26) < 2 && abs(bounds.height - 26) < 2, "Picker X must be 26px square")
    // Use the labeled AX control after filtering rather than a second geometry lookup.
    require(AXUIElementPerformAction(close, kAXPressAction as CFString) == .success, "Cannot press picker X")
    Thread.sleep(forTimeInterval: 1)
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
// Saved reply declaration, edited through File view (the existing in-memory fixture writer) so it works while the
// picker blocks adding the tool. Each size ends with the original file text restored.
}
func fileEditor() -> AXUIElement {
    let found = nativeElements().filter { attribute($0, "AXRole") as? String == "AXTextArea" && (attribute($0, "AXValue") as? String)?.hasPrefix("---") == true }
    require(found.count == 1, "Expected the File view editor")
    return found[0]
}
func currentFileText() -> String {
    scroll(4000); click("File", [])
    let text = attribute(fileEditor(), "AXValue") as? String ?? ""
    click("Agent", [])
    return text
}
func setFileText(_ text: String) {
    scroll(4000); click("File", [])
    let editor = fileEditor()
    require(AXUIElementSetAttributeValue(editor, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "Cannot focus File view")
    require(AXUIElementSetAttributeValue(editor, kAXValueAttribute as CFString, text as CFString) == .success, "Cannot set File view text")
    // A real key pair makes the page observe the edit whichever way AX set the value.
    typeUnicode(" ")
    for down in [true, false] { CGEvent(keyboardEventSource: nil, virtualKey: 51, keyDown: down)?.post(tap: .cghidEventTap) }
    Thread.sleep(forTimeInterval: 0.5)
    require(attribute(editor, "AXValue") as? String == text, "File view text was not applied")
    scroll(4000) // Editing can scroll the caret into view below the header.
    click("Agent", []) // Leaving File view saves in memory.
    Thread.sleep(forTimeInterval: 1)
}
func visibleStaticTexts(_ matches: (String) -> Bool) -> Bool {
    nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXStaticText", let text = attribute(element, "AXValue") as? String, matches(text),
            let bounds = nativeBounds(element) else { return false }
        return bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)
    }
}
func replyChipVisible() -> Bool { visibleStaticTexts { $0 == "reply_in_conversation" } }
// The warning's wording is shared with other tools; require only that it names this tool beyond the chip itself.
func replyWarningVisible() -> Bool { visibleStaticTexts { $0.contains("reply_in_conversation") && $0 != "reply_in_conversation" } }
func requireSavedReply(_ present: Bool, _ label: String) {
    Thread.sleep(forTimeInterval: 0.5)
    require(replyChipVisible() == present, "\(label): reply chip should be \(present ? "present" : "absent")")
    require(replyWarningVisible() == present, "\(label): saved reply warning should be \(present)")
}
if scope.runsModel {
let originalFile = currentFileText()
let toolAnchor = "      name: web_fetch\n"
require(originalFile.contains(toolAnchor), "Fixture agent must declare web_fetch")
let replyFile = originalFile.replacingOccurrences(of: toolAnchor, with: toolAnchor + "    - kind: builtin\n      name: reply_in_conversation\n")
for (width, height, size) in [(1280, 800, "wide"), (900, 800, "medium"), (540, 900, "compact")] {
    localSize(CGFloat(width), CGFloat(height))
    setFileText(replyFile)
    _ = try seek("It can use", -150)
    requireSavedReply(true, "preserved \(size)")
    _ = try capture("local-saved-reply-tool-\(size)", ["It can use", "Reply in a conversation"])
    click("Remove Reply in a conversation", [])
    requireSavedReply(false, "removed \(size)")
    _ = try capture("local-saved-reply-tool-removed-\(size)", ["It can use"])
    setFileText(replyFile)
    _ = try seek("It can use", -150)
    requireSavedReply(true, "restored \(size)")
    _ = try capture("local-saved-reply-tool-restored-\(size)", ["It can use", "Reply in a conversation"])
    setFileText(originalFile)
}
scroll(4000)
// Match the known prompt, ignoring separator blank lines exposed by File edits,
// not a label plus a guessed offset after responsive reflow.
let editors = nativeElements().filter {
    attribute($0, "AXRole") as? String == "AXTextArea"
        && (attribute($0, "AXValue") as? String)?.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("Answer from the web page named in the request.") == true
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
require(exactTextIsVisible("Saved"), "A completed edit/save must acknowledge Saved")
// Focusing Instructions scrolls below the compact test card; return to its start.
scroll(4000)
_ = try seek("Not run since your edit", -250)
_ = try capture("local-stale", ["Not run since your edit"])
// Shell evidence beside the approved Designer frame: Agent, File, then Send and Past Sends opened on request.
// Blocked Chat is captured in the remote section above. The first (wide) pass sees the unused Send hidden; opening it
// keeps it retained, so the minimum pass must see it already visible.
}
func exactTextIsVisible(_ expected: String) -> Bool {
    nativeElements().contains { element in
        guard attribute(element, "AXRole") as? String == "AXStaticText", attribute(element, "AXValue") as? String == expected,
            let bounds = nativeBounds(element) else { return false }
        return bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)
    }
}
if scope.runsModel {
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    click("Agent", [])
    require(nativeElements().contains { attribute($0, "AXRole") as? String == "AXPopUpButton" }, "Header agent picker missing")
    require(anyButton("Send to a cluster") && anyButton("Past Sends"), "Header Send entry and Past Sends must always exist")
    if size == "wide" { require(!anyButton("Choose a cluster"), "An unused Send must start hidden") }
    else { require(anyButton("Choose a cluster"), "Send opened earlier must stay retained and visible") }
    _ = try capture("local-shell-agent-\(size)", ["Instructions and settings"])
    click("File", [])
    _ = try capture("local-shell-file-\(size)", ["web-reader.agent.md"])
    click("Agent", [])
    click("Send to a cluster", [])
    // The header action already reveals and scrolls the card into view.
    require(exactTextIsVisible("Send your agent") && !nativeChatTextIsVisible("Team clusters are early in this preview."), "Send must retain its headline without repeating Welcome's preview sentence")
    require(enabledButton("Choose a cluster") != nil && !anyButton("Send and run tests"), "Without a cluster, Choose a cluster must replace Send outside Details")
    _ = try capture("local-shell-send-\(size)", ["Send your agent", "Choose a cluster"])
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
}
func requireSingleChatCopy() {
    let strings = nativeElements().flatMap { element in
        ["AXValue", "AXTitle", "AXDescription"].compactMap { attribute(element, $0) as? String }
    }
    require(strings.contains(where: { $0.contains("Chat on this computer. Chat doesn't change test results.") }), "Chat subtitle missing from the agent heading")
    require(!strings.contains(where: { $0.contains("Conversation 1") }), "A single Chat version must not be numbered, including its accessible label")
}
func requireChatComposerVisible() {
    let fields = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXTextArea"
            && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == "Message" })
    }
    let sends = nativeElements().filter { element in
        attribute(element, "AXRole") as? String == "AXButton"
            && ["AXTitle", "AXDescription"].contains(where: { attribute(element, $0) as? String == "Send message" })
    }
    require(fields.count == 1 && sends.count == 1, "Chat must retain accessible Message and Send message controls")
    guard let field = fields.first.flatMap(nativeBounds), let send = sends.first.flatMap(nativeBounds) else {
        require(false, "Chat composer bounds missing"); return
    }
    require(reachable(field) && reachable(send) && abs(field.maxY - send.maxY) < 3 && field.maxX <= send.minX,
            "Chat composer and visible Send must fit beside one another above the footer")
    require(nativeChatTextIsVisible("Enter sends · Shift+Enter for a new line", aboveFooter: true), "Chat keyboard hint must remain visible")
}
if scope.runsModel {
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    requireSingleChatCopy()
    require(!anyButton("New conversation") && !nativeTextExists("About this conversation")
            && !nativeTextExists("Ask your agent a question") && !nativeTextExists("Ready for your message."), "Healthy empty Chat must not show controls, invitation or routine Ready")
    requireChatComposerVisible()
    _ = try capture("chat-empty-\(size)", ["Message your agent", "Enter sends · Shift+Enter for a new line"])
}
fillMessage("Remember the checksum ALPHA.")
waitForButton("Send message")
click("Send message", [])
localSize(1280, 800); scroll(4000)
_ = try seek("Waiting for your agent to start…", -100)
waitForChatText("Waiting for your agent to start…")
for (phase, expected) in [("pending", "Waiting for your agent to start…"), ("running", "Waiting for your agent’s reply…"), ("session-null", "Waiting for the conversation to finish saving…"), ("session-answer", "The checksum is ALPHA.")] {
    try phase.write(to: chatPhase, atomically: true, encoding: .utf8)
    waitForChatText(expected)
    for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
        localSize(CGFloat(width), CGFloat(height)); scroll(4000)
        _ = try seek(expected, -100)
        _ = try capture("chat-\(phase)-\(size)", [expected])
        require(!nativeTextExists("This turn is not confirmed in later context.") && !nativeTextExists("No answer was received."), "Waiting Chat must not show a premature missing-answer/context fault")
        if phase == "session-answer" {
            require(nativeChatTextIsVisible("The checksum is ALPHA."), "Session wait must retain the known answer")
        }
    }
}
try "answer".write(to: chatPhase, atomically: true, encoding: .utf8)
waitForChatText("The checksum is ALPHA.")
try FileManager.default.removeItem(at: chatPhase)
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("The checksum is ALPHA.", -100)
    requireSingleChatCopy()
    requireChatComposerVisible()
    require(nativeChatTextIsVisible("web_fetch"), "Chat tool use must remain visible without opening a disclosure")
    _ = try capture("chat-answer-\(size)", ["The checksum is ALPHA.", "<script>fixture</script>", "web_fetch"])
}
localSize(1280, 800)
// Use the existing synthetic native turn, not injected DOM or a new fixture backend.
fillMessage("What checksum did I ask you to remember?" + String(repeating: "\nEarlier message.", count: 40))
waitForButton("Send message")
click("Send message", [])
waitForChatText("You asked me to remember ALPHA.")
_ = try capture("chat-followup-wide", ["You asked me to remember ALPHA."])
localSize(540, 900)
_ = try capture("chat-followup-compact", ["You asked me to remember ALPHA."])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("Remember the checksum ALPHA.", 4000, diagnosticPrefix: "chat-long-early-\(size)-seek")
    require(nativeChatTextIsVisible("Remember the checksum ALPHA.") && !nativeChatTextIsVisible("You asked me to remember ALPHA."),
            "Long transcript capture must read earlier messages, not the end")
    requireChatComposerVisible()
    _ = try capture("chat-long-early-\(size)", ["Remember the checksum ALPHA.", "Enter sends · Shift+Enter for a new line"])
    scroll(-4000)
    requireChatComposerVisible()
    _ = try capture("chat-long-end-\(size)", ["You asked me to remember ALPHA.", "Enter sends · Shift+Enter for a new line"])
}
localSize(1280, 800); scroll(4000)
click("New conversation", [])
scroll(-4000)
fillMessage("Read another page.")
waitForButton("Send message")
rows = try seek("Enter sends · Shift+Enter for a new line", -150)
click("Send message", rows)
waitForChatText("Read another page.")
require(nativeHeadingExists("Conversation 1") && nativeHeadingExists("Conversation 2"), "Two Chat versions must retain both numbered headings")
localSize(540, 640)
anchorChatHeading("Conversation 2")
_ = try capture("chat-new-version-minimum", ["Conversation 2", "Read another page."])
localSize(1280, 800)
anchorChatHeading("Conversation 2")
_ = try capture("chat-new-version-wide", ["Conversation 2", "Read another page."])
require(enabledButton("Stop") != nil, "Working Chat must offer Stop")
_ = try capture("chat-working-wide", ["Stop", "Read another page."])
localSize(540, 900)
_ = try capture("chat-working-compact", ["Stop", "Read another page."])
click("Stop", [])
waitForChatText("Execution termination is unconfirmed.")
waitForButton("New conversation")
_ = try capture("chat-stop-ready-compact", ["Execution termination is unconfirmed."])
localSize(1280, 800)
_ = try capture("chat-stop-ready-wide", ["Execution termination is unconfirmed."])
// Capture list actions before the final test deliberately holds the fixture busy.
click("Overview", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 900, "compact"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try capture("local-controls-overview-collapsed-\(size)", ["Your agents", "web-reader"])
    for (agent, scene, expected) in [
        ("release-notes", "earlier-pass", ["Not run since your edit", "2 of 2 passed, before your edit", "Run tests"]),
        ("pr-summary", "earlier-failed", ["Failed", "1 of 2 passed, before your edit", "Run tests"]),
        ("draft-helper", "draft", ["Draft", "Add instructions and a test to get started."])
    ] {
        scroll(4000)
        setOverviewExpanded(agent, true)
        if size == "minimum" { scroll(-150) }
        _ = try seek(scene == "draft" ? expected[0] : expected[1], -150)
        let proof = try capture("local-controls-overview-expanded-\(scene)-\(size)", expected)
        let pixels = proof.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        require(expected.allSatisfy { pixels.contains($0) }, "Expanded Overview result must be visible in OCR")
        scroll(4000)
        setOverviewExpanded(agent, false)
    }
}
localSize(1280, 800); scroll(4000)
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
_ = try capture("local-manage-compact", ["This computer", "Change"])
// Keep the completed local and removal states reviewable at the compact size too.
_ = try seek("Change", -200)
click("Change", [])
click("Remove saved sign-in", [])
_ = try capture("local-remove-confirmation-compact", ["saved sign-in?", "Cancel", "Remove sign-in"])
click("Cancel", [])
click("Done", [])
click("Clusters", [])
_ = try capture("local-places-compact", ["Choose where agents run", "This computer"])
}

if scope.runsDestinations {
if scope == .destinationsWorkspace {
    // Fresh defaults: selected/ready local workspace, no installing/returning reset.
    localSize(1280, 800)
    press("Clusters")
    if anyButton("Get started") { press("Get started") }
    if !anyButton("Add a cluster") { press("Clusters") }
    waitForButton("Add a cluster")
    require(!FileManager.default.fileExists(atPath: setupProgress.path), "Allocated bootstrap must not write setup progress before its journey")
}
// Explicit context actions only: opening the picker never starts discovery.
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try capture("cluster-list-\(size)", ["Choose where agents run", "This computer", "team-dev"])
}
localSize(1280, 800); scroll(4000)
click("Add a cluster", [])
click("kubectl context", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height))
    _ = try capture("cluster-picker-open-\(size)", ["Find a context", "team-dev", "team-west"])
}
click("Close picker", [])
localSize(1280, 800); scroll(4000)
click("kubectl context", [])
try "cluster-checking".write(to: setupProgress, atomically: true, encoding: .utf8)
click("team-west", [])
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try capture("cluster-checking-\(size)", ["Checking"])
}
try "cluster-failure".write(to: setupProgress, atomically: true, encoding: .utf8)
waitForChatText("Can't reach team-west. Check your network or VPN.")
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try capture("cluster-failure-\(size)", ["Can't reach team-west.", "Check your network or VPN."])
    if size == "minimum" {
        _ = try seek("Check again", -100)
        guard let bounds = enabledButton("Check again").flatMap(nativeBounds) else { fatalError("Missing recovery action") }
        require(window()!.1.contains(bounds) && bounds.maxY <= window()!.1.maxY - 32, "Recovery action overlaps the footer")
        _ = try capture("cluster-failure-minimum-end", ["Check again"])
    }
}

// Real picker/search/close/row actions against bounded synthetic account IPC.
localSize(1280, 800)
try "cluster-account-picker".write(to: setupProgress, atomically: true, encoding: .utf8)
press("Check again")
waitForButton("Use this cluster"); press("Use this cluster")
waitForButton("ServiceAccount"); press("ServiceAccount")
_ = try capture("cluster-account-picker-ready", ["Find an account"])
waitForButton("task-reader")
fillMessage("team", fieldLabel: "Find an account")
require(!anyButton("task-reader") && anyButton("team-runner"), "Account search must filter names")
click("Close picker", [])
press("ServiceAccount")
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height))
    _ = try capture("cluster-account-picker-open-\(size)", ["Find an account", "team-runner", "task-reader", "Not checked"])
}
click("team-runner", [])
localSize(1280, 800)
_ = try seek("Required Orka permissions: Allowed", -150)
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("Required Orka permissions", -100)
    _ = try capture("cluster-account-allowed-\(size)", ["team-runner", "Required Orka permissions: Allowed", "Connection and broader access not verified."])
}
// A fresh context retires the prior selection before injecting the finite list403.
localSize(1280, 800)
try "cluster-account-forbidden".write(to: setupProgress, atomically: true, encoding: .utf8)
press("kubectl context"); click("team-dev", [])
waitForButton("Use this cluster"); press("Use this cluster")
_ = try seek("ServiceAccount name", -100)
fillMessage("team-runner", fieldLabel: "ServiceAccount name")
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("ServiceAccount name", -100)
    _ = try capture("cluster-account-forbidden-\(size)", ["cannot list ServiceAccounts in agents", "ServiceAccount name"])
}
// Saved-file editing without a usable destination must not request/blame a catalog.
try "designer-no-destination".write(to: setupProgress, atomically: true, encoding: .utf8)
localSize(1280, 800)
press("Overview"); press("Open web-reader")
for (width, height, size) in [(1280, 800, "wide"), (540, 640, "minimum")] {
    localSize(CGFloat(width), CGFloat(height)); scroll(4000)
    _ = try seek("Connect a cluster in Clusters", -100)
    _ = try capture("local-catalog-no-destination-\(size)", ["Connect a cluster in Clusters to load tools and models"])
    require(!nativeTextExists("Couldn't read tools from") && !nativeTextExists("Couldn't read models from"), "No prior destination may be blamed")
}
try FileManager.default.removeItem(at: setupProgress)

func captureUpgradeMatrix(manage: Bool) throws {
    let rows: [(String, String, String?)] = manage ? [
        ("sourceSelected", "Workspace setup was interrupted", nil),
        ("sourceRetained", "Workspace setup was interrupted", nil),
        ("restoring", "Restore was interrupted", "Restore previous workspace"),
        ("fresh-absent", "Enter a new sign-in or key", nil),
        ("fresh-eligible", "A saved connection can be reused", nil),
        ("fresh-attention", "The saved connection needs attention", "Remove saved connection"),
        ("fresh-transferred", "Saved connection ready", nil),
        ("fresh-removed", "Enter a new sign-in or key", nil),
        ("consumed", "Connect the AI explicitly", "Restore previous workspace"),
        ("restored", "Previous workspace restored", "Set up another new workspace"),
        ("conflict", "Workspace recovery needs attention", nil),
        ("eligible-offline", "Check your internet connection", nil),
        ("eligible-nospace", "not enough free space", nil),
        ("incomplete-stop", "could not be stopped safely", nil),
        ("busy", "Updating the workspace", nil),
    ] : [
        ("eligible-absent", "This version of Bridge needs a new workspace", "Set up new workspace"),
        ("eligible-credential", "This version of Bridge needs a new workspace", "Set up new workspace"),
        ("incomplete", "Workspace setup was interrupted", "Try setup again"),
    ];
    for (index, row) in rows.enumerated() {
        let (state, expected, confirmation) = row
        let phase = "upgrade-\(state)\(manage ? "-manage" : "")"
        try phase.write(to: setupProgress, atomically: true, encoding: .utf8)
        if manage && index == 0 { waitForButton("Manage"); click("Manage", []) }
        waitForNativeText([expected])
        waitForNativeText([manage ? "Agents" : "Where agents run"])
        if state == "eligible-credential" {
            waitForNativeText(["A saved connection can be reused without connecting automatically."])
        }
        if state == "sourceRetained" {
            // Identical recovery prose cannot prove this scene reached React.
            // Observe its synthetic transaction in real rendered native details.
            press("Update details")
            waitForNativeText(["4000…0005"])
            click("Update details", [])
            var closed = false
            for _ in 0..<40 {
                if !nativeTextExists("4000…0005") { closed = true; break }
                Thread.sleep(forTimeInterval: 0.1)
            }
            require(closed, "Upgrade phase witness details must close before capture")
        }
        for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("compact", CGFloat(540), CGFloat(900))] {
            localSize(width, height); scroll(4000)
            _ = try seek(expected, -100)
            if state == "eligible-absent" {
                require(!nativeTextExists("A saved connection can be reused without connecting automatically."), "Absent credential must not show saved-connection copy")
            } else if state == "eligible-credential" {
                require(nativeTextExists("A saved connection can be reused without connecting automatically."), "Eligible credential must show saved-connection copy")
            }
            _ = try capture("local-upgrade-\(manage ? "manage" : "setup")-\(state)-\(size)", [expected])
            if state == "busy" {
                require(enabledButton("Cancel update") != nil, "An update must remain cancellable")
                require(enabledButton("Restore previous workspace") == nil, "Busy must not authorize restore")
            }
            if let action = confirmation {
                press(action)
                require(enabledButton("Not now") != nil, "Confirmation must offer Not now")
                reveal("Not now")
                guard let notNow = enabledButton("Not now") else { fatalError("Not now missing") }
                print("Upgrade focus before AX:", attribute(notNow, "AXFocused") as? Bool ?? false, String(describing: nativeBounds(notNow)), reachable(nativeBounds(notNow)))
                require(AXUIElementSetAttributeValue(notNow, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "Cannot focus safe confirmation action")
                print("Upgrade focus after AX:", attribute(notNow, "AXFocused") as? Bool ?? false)
                // Specify modifiers on every event and release Shift, so one scene cannot modify the next Tab.
                for backwards in [false, true] {
                    for down in [true, false] {
                        let event = CGEvent(keyboardEventSource: nil, virtualKey: 48, keyDown: down)
                        event?.flags = backwards && down ? .maskShift : []
                        event?.post(tap: .cghidEventTap)
                    }
                }
                Thread.sleep(forTimeInterval: 0.2)
                // AX nodes can be refreshed by polling; observe the current node without extending the key deadline.
                let currentFocus = enabledButton("Not now").map { attribute($0, "AXFocused") as? Bool == true } == true
                if !currentFocus {
                    _ = try capture("local-probe", [])
                    let focused = nativeElements().filter { attribute($0, "AXFocused") as? Bool == true }
                    print("Upgrade confirmation focus:", focused.map { attribute($0, "AXTitle") as? String ?? attribute($0, "AXRole") as? String ?? "unavailable" })
                }
                require(currentFocus, "Confirmation keyboard focus was lost")
                let warning = action == "Restore previous workspace" ? "cannot be reopened" : action == "Remove saved connection" ? "every retained workspace" : state == "restored" ? "will not be reopened" : "Do not open the older version"
                scroll(4000); _ = try seek(warning, -100)
                _ = try capture("local-upgrade-\(manage ? "manage" : "setup")-\(state)-confirm-\(size)", [warning])
                press("Not now")
                require(enabledButton(action) != nil, "Not now must leave the original action available")
            }
        }
    }
    localSize(1280, 800); scroll(4000)
}
// Capture setup upgrade states before the returning-scene reload retires first-test intent.
localSize(1280, 800); scroll(4000)
click("Clusters", [])
click("Clusters", [])
waitForButton("Connect the AI"); click("Connect the AI", [])
press("Connect the AI")
_ = try seek("Connected", -100)
try captureUpgradeMatrix(manage: false)
try FileManager.default.removeItem(at: setupProgress)

// Explicit scenes use only the existing synthetic fixture's IPC owner.
func workspacePhase(_ phase: String) throws {
    try phase.write(to: setupProgress, atomically: true, encoding: .utf8)
    Thread.sleep(forTimeInterval: 1.2)
}
func workspaceCapture(_ state: String, _ expected: [String]) throws {
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        localSize(width, height); scroll(4000)
        _ = try capture("local-workspace-\(state)-\(size)", expected)
        require(enabledButton("Stop the workspace") == nil && enabledButton("Start") == nil && enabledButton("Cancel") == nil, "Returning workspace must not expose Start/Stop/Cancel")
        if ["failed", "stopped-unexpectedly", "stop-failed", "forced-stop"].contains(state) {
            require(!anyButton("Hide details"), "Diagnostics must start folded")
            click("Show details", [])
            _ = try capture("local-workspace-\(state)-details-\(size)", state == "stopped-unexpectedly" ? ["Synthetic workspace stopped unexpectedly.", "Hide details"] : ["Hide details"])
            click("Hide details", [])
        }
    }
    localSize(1280, 800); scroll(4000)
}
// First scene reloads once with selected localVm plus the attempted marker.
try workspacePhase("workspace-scene-ready")
waitForButton("Manage"); click("Manage", [])
try workspaceCapture("ready", ["This computer", "Ready", "Agents", "Files"])
require(nativeHeadingExists("AI"), "Returning Model must be headed AI")
click("What's inside", [])
_ = try capture("local-workspace-inside", ["4 GB of memory"])
click("What's inside", [])
try workspacePhase("workspace-scene-starting")
try workspaceCapture("starting-after-quit-synthetic", ["This computer", "Starting", "0:18 so far"])
click("Designer", [])
for (view, label) in [("Agent", "tests"), ("Chat", "chat")] {
    localSize(1280, 800); scroll(4000)
    click(view, [])
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
        localSize(width, height); scroll(view == "Chat" ? -12000 : 4000)
        _ = try seek("Waiting for the workspace to start.", -100)
        _ = try capture("local-designer-waiting-\(label)-\(size)", ["Waiting for the workspace to start."])
    }
}
click("Clusters", [])
try workspacePhase("workspace-scene-failed")
try workspaceCapture("failed", ["This computer", "Couldn't start", "Restart workspace"])
try workspacePhase("workspace-scene-stopped-unexpectedly")
try workspaceCapture("stopped-unexpectedly", ["This computer", "Your workspace didn't start.", "Restart workspace"])
try workspacePhase("workspace-scene-stop-failed")
try workspaceCapture("stop-failed", ["This computer", "The workspace didn't stop.", "Quit Bridge"])
require(enabledButton("Restart workspace") == nil, "Unconfirmed Stop must not authorize restart")
try workspacePhase("workspace-scene-forced-stop")
try workspaceCapture("forced-stop", ["This computer", "Forcibly stopped", "may be lost", "Quit Bridge"])
require(enabledButton("Restart workspace") == nil, "Forced Stop must not authorize restart")
try workspacePhase("workspace-scene-closed")
try workspaceCapture("closed", ["This computer", "Stopped", "Quit Bridge"])
try captureUpgradeMatrix(manage: true)
// New owner-contract scenes extend the existing destination journey once, not a
// setup×Manage combinatorial duplicate. Every action is synthetic fixture IPC.
func integrationUpgradeShots(_ state: String, _ expected: String) throws {
    for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("compact", CGFloat(540), CGFloat(900))] {
        localSize(width, height); scroll(4000)
        reveal("Update details")
        guard let details = enabledButton("Update details") else { fatalError("Update details missing") }
        require(AXUIElementSetAttributeValue(details, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "Cannot focus upgrade details")
        for backwards in [false, true] {
            for down in [true, false] {
                let event = CGEvent(keyboardEventSource: nil, virtualKey: 48, keyDown: down)
                event?.flags = backwards && down ? .maskShift : []
                event?.post(tap: .cghidEventTap)
            }
        }
        Thread.sleep(forTimeInterval: 0.2)
        require(enabledButton("Update details").map { attribute($0, "AXFocused") as? Bool == true } == true, "Integration keyboard focus was lost")
        scroll(4000); _ = try seek(expected, -100)
        _ = try capture("local-upgrade-manage-\(state)-\(size)", [expected])
    }
}
for state in ["refresh-pending", "refresh-busy", "refresh-io", "cancel-requested", "cancel-completed", "cancel-not-cancellable", "forward-cancelled", "busy-locked", "incomplete-forced"] {
    localSize(1280, 800); scroll(4000)
    try workspacePhase("upgrade-\(state)-manage")
    // Each finite interactive phase remounts with retired first-test intent.
    waitForButton("Manage"); click("Manage", [])
    var expected = state == "incomplete-forced" ? "forcibly stopped" : state == "busy-locked" ? "The workspace is busy. Your agent files are kept." : state.hasPrefix("cancel-") ? "Cancel update" : state == "forward-cancelled" ? "Try setup again" : "Check again"
    if state.hasPrefix("cancel-") || state == "forward-cancelled" || state.hasPrefix("refresh-") { waitForButton(expected) } else { waitForNativeText([expected]) }
    if state.hasPrefix("refresh-") {
        waitForNativeText(["1.2 GB"])
        require(!nativeTextExists("Workspace files checked on request."), "Cached observation must not claim owner freshness")
        if state == "refresh-pending" { try integrationUpgradeShots("refresh-cached", "Check again") }
        press("Check again")
        expected = state == "refresh-pending" ? "Checking workspace" : "Could not check workspace files. They may still be in use or unavailable. Check again before continuing."
        waitForNativeText([expected])
        if state != "refresh-pending" {
            // Observe React committing the next cached DTO, not elapsed time or a request.
            // Bytes are fixture metadata only; transaction/history and failed I/O stay unchanged.
            waitForNativeText(["1.3 GB"])
            require(nativeTextExists(expected), "Cached observation erased manual refresh error")
            require(!nativeTextExists("Workspace files checked on request."), "Cached observation must not claim owner freshness")
        }
    } else if state.hasPrefix("cancel-") {
        press("Cancel update")
        // A terminal cache observation may beat the bare completed reply. Capture
        // its stable recovery state, not an acknowledgement that correctly expires.
        expected = state == "cancel-requested" ? "Cancellation requested. Waiting for the workspace operation to stop." : state == "cancel-completed" ? "Workspace setup was interrupted. Choose Try setup again to continue. Reopen this version of Bridge if recovery is interrupted again." : "This workspace operation can no longer be cancelled. Check the latest status."
        waitForNativeText([expected])
        if state == "cancel-completed" {
            require(!anyButton("Cancel update") && enabledButton("Try setup again") != nil, "Completed cancellation must expose terminal recovery, not a stale waiting acknowledgement")
        } else {
            // ACK text can precede the cached capability render. Keep the existing
            // four-second polling budget and observe AX, not a future DTO.
            for _ in 0..<40 {
                if !anyButton("Cancel update") { break }
                Thread.sleep(forTimeInterval: 0.1)
            }
            require(!anyButton("Cancel update"), "Acknowledged cancellation must withdraw the rendered capability")
            if state == "cancel-requested" {
                require(anyButton("Restore previous workspace") && enabledButton("Restore previous workspace") == nil, "Busy cancellation must retain disabled restore")
            }
            require(enabledButton("Try setup again") == nil && enabledButton("Restore previous workspace") == nil, "Cancellation acknowledgement must not release the update fence")
        }
    } else if state == "forward-cancelled" {
        press("Try setup again")
        press("Keep old workspace and set up new")
        expected = "Update cancelled"
        waitForNativeText([expected])
    } else if state == "busy-locked" {
        require(!anyButton("Cancel update"), "Busy is not cancellation authority")
    }
    try integrationUpgradeShots(state, expected)
}
for (phase, summary) in [("image-update", "This version of Bridge needs a new workspace."), ("image-unsupported", "This workspace uses different cluster files.")] {
    try workspacePhase(phase)
    try workspaceCapture(phase, [summary, "Restart workspace"])
    require(enabledButton("Restart workspace") == nil && !anyButton("Set up new workspace"), "Image refusal must not retry or replace the workspace")
}
try workspacePhase("setup-forced-stop")
if !nativeTextExists("Get your workspace ready") {
    click("Clusters", [])
    let action = ["Continue setup", "View details", "Set up"].first { enabledButton($0) != nil }
    require(action != nil, "Missing first-run recovery action")
    click(action!, [])
}
for (size, width, height) in [("wide", CGFloat(1280), CGFloat(800)), ("minimum", CGFloat(540), CGFloat(640))] {
    localSize(width, height); scroll(4000)
    _ = try capture("local-workspace-setup-forced-stop-\(size)", ["may be lost", "Quit Bridge"])
    require(enabledButton("Quit Bridge") != nil && !anyButton("Try again") && !anyButton("Restart workspace"), "First-run forced stop must offer Quit without retry")
}
try "Renderer-only synthetic state captures, not VM lifecycle proof. Native Ready close notice, menu-bar dot and real quit/reopen startup remain unverified.\n".write(toFile: "\(output)/workspace-capture-gaps.txt", atomically: true, encoding: .utf8)
}

try coverage.finish()
let scopeResult: [String: Any] = ["scope": scope.rawValue, "names": coverage.names]
try JSONSerialization.data(withJSONObject: scopeResult, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: output).appendingPathComponent("scope-manifest.json"))
print("PASS native scope \(scope.rawValue): \(coverage.names.count) unique named captures")
