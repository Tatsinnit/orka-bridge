import AppKit
import CoreGraphics
import Vision

func require(_ condition: Bool, _ message: String) {
    if !condition { fputs("\(message)\n", stderr); exit(1) }
}
let output = CommandLine.arguments[1]
let theme = CommandLine.arguments[3]
require(["light", "dark"].contains(theme), "Expected light or dark theme")
require(NSRunningApplication.runningApplications(withBundleIdentifier: "ai.orka.bridge").isEmpty,
        "Quit running Bridge before running the native walk")
let opened = Process()
opened.executableURL = URL(fileURLWithPath: "/usr/bin/open")
opened.arguments = [CommandLine.arguments[2]]
try opened.run()
opened.waitUntilExit()
require(opened.terminationStatus == 0, "Cannot open Bridge")
var running: NSRunningApplication?
for _ in 0..<30 {
    running = NSRunningApplication.runningApplications(withBundleIdentifier: "ai.orka.bridge").first
    if running != nil { break }
    Thread.sleep(forTimeInterval: 1)
}
guard let app = running else { fatalError("Bridge process did not appear") }
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
var found = false
for _ in 0..<30 {
    if window() != nil { found = true; break }
    Thread.sleep(forTimeInterval: 1)
}
require(found, "Bridge native window did not appear")
Thread.sleep(forTimeInterval: 2)
func mouse(_ type: CGEventType, _ point: CGPoint) {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
}
func click(_ fraction: CGFloat) {
    let rect = window()!.1
    let point = CGPoint(x: rect.minX + rect.width * fraction, y: rect.minY + 55)
    mouse(.leftMouseDown, point); mouse(.leftMouseUp, point)
    Thread.sleep(forTimeInterval: 0.5)
}
func key(_ code: CGKeyCode, shift: Bool = false) {
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)!
        event.flags = shift ? .maskShift : []
        event.post(tap: .cghidEventTap)
    }
    Thread.sleep(forTimeInterval: 0.3)
}
func capture(_ name: String, _ page: String) throws {
    let file = "\(output)/\(name).png"
    let command = Process()
    command.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    // Exclude the window shadow so OCR coordinates are relative to the window.
    command.arguments = ["-x", "-o", "-l", String(window()!.0), file]
    try command.run(); command.waitUntilExit()
    require(command.terminationStatus == 0, "Cannot capture Bridge")
    guard let bitmap = NSBitmapImageRep(data: try Data(contentsOf: URL(fileURLWithPath: file))),
        let colour = bitmap.colorAt(x: bitmap.pixelsWide - 20, y: bitmap.pixelsHigh - 20)?.usingColorSpace(NSColorSpace.deviceRGB)
        else { fatalError("Cannot sample native screenshot") }
    let brightness = (colour.redComponent + colour.greenComponent + colour.blueComponent) / 3
    require(theme == "dark" ? brightness < 0.2 : brightness > 0.9,
            "Native screenshot does not show the expected \(theme) theme")
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    try VNImageRequestHandler(url: URL(fileURLWithPath: file)).perform([request])
    let observations = request.results ?? []
    let pages = ["Overview", "Designer", "Clusters"]
    let headings = Set(observations.compactMap { observation -> String? in
        let top = 1 - observation.boundingBox.maxY
        guard top > 0.10 && top < 0.25,
            let text = observation.topCandidates(1).first?.string,
            pages.contains(text) else { return nil }
        return text
    })
    // Negative control: persistent nav labels must not qualify as page headings.
    for wrong in pages where wrong != page {
        require(!headings.contains(wrong), "OCR confused the \(wrong) nav label with a heading")
    }
    require(headings == Set([page]), "Native screenshot does not show the expected \(page) page heading; observed \(headings)")
    print("PASS \(name): native \(page) heading observed")
}
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
try capture("overview", "Overview")
click(0.55); try capture("designer-pointer", "Designer")
click(0.85); try capture("clusters-pointer", "Clusters")
key(48, shift: true); key(48, shift: true); key(49)
try capture("overview-keyboard", "Overview")
key(48); key(49); try capture("designer-keyboard", "Designer")
key(48); key(36); try capture("clusters-keyboard", "Clusters")
// Resize this CI-owned window using its native bottom-right handle.
let rect = window()!.1
// The display may constrain the initial size. Derive the minimum frame from
// AppKit's content-to-frame conversion, not the requested initial dimensions.
let minimumFrame = NSWindow.frameRect(
    forContentRect: CGRect(x: 0, y: 0, width: 960, height: 640),
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
print("PASS native walk: pointer, Shift-Tab/Tab, Space, Enter and minimum window")
