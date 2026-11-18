import Foundation
import Virtualization
import Darwin

private var failures = 0
private var assertions = 0
private func check(_ condition: @autoclosure () -> Bool, _ message: String, line: Int = #line) {
    assertions += 1
    if !condition() { failures += 1; print("FAIL line \(line): \(message)") }
}
private func frame(_ op: UInt8, _ id: UInt64) -> [UInt8] {
    [op] + (0..<8).map { UInt8(truncatingIfNeeded: id >> ($0 * 8)) }
}
private final class FakeMachine: MachineDriver {
    var state = MachineState.stopped
    var canStart = true
    var canPause = true
    var canResume = true
    var canStop = true
    var didEnd: (() -> Void)?
    var calls: [String] = []
    var callback: ((Bool) -> Void)?
    var disposeCheck: (() -> Void)?
    func start(_ completion: @escaping (Bool) -> Void) { calls.append("start"); callback = completion }
    func pause(_ completion: @escaping (Bool) -> Void) { calls.append("pause"); callback = completion }
    func resume(_ completion: @escaping (Bool) -> Void) { calls.append("resume"); callback = completion }
    func stop(_ completion: @escaping (Bool) -> Void) { calls.append("stop"); callback = completion }
    func dispose() { calls.append("dispose"); disposeCheck?() }
    func complete(_ state: MachineState, _ success: Bool = true) {
        self.state = state
        let completion = callback; callback = nil
        completion?(success)
    }
    func ended(_ state: MachineState) { self.state = state; didEnd?() }
}
private final class Trace {
    var writes: [[UInt8]] = []
    var terminal: [Bool] = []
    var outputWorks = true
    var disposedWithOpenFD = false
    var repliesWithClosedFD: [Bool] = []
}
private final class Harness {
    let driver = FakeMachine()
    let trace = Trace()
    let fd: Int32
    let original: FileHandle
    let url: URL
    let core: BridgeVM
    init(_ queue: DispatchQueue) {
        url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        precondition(FileManager.default.createFile(atPath: url.path, contents: Data(), attributes: [.posixPermissions: 0o600]))
        original = try! FileHandle(forUpdating: url)
        fd = dup(original.fileDescriptor)
        precondition(fd >= 0)
        let t = trace, ownedFD = fd
        driver.disposeCheck = { t.disposedWithOpenFD = fcntl(ownedFD, F_GETFD) >= 0 }
        core = BridgeVM(driver: driver, ownershipFD: fd, queue: queue, output: { bytes in
            t.writes.append(bytes)
            if bytes.first == 75 || bytes.first == 78 { t.repliesWithClosedFD.append(fcntl(ownedFD, F_GETFD) == -1) }
            return t.outputWorks
        }, terminal: { status in
            check(fcntl(ownedFD, F_GETFD) == -1, "terminal before ownership closed")
            t.terminal.append(status)
        })
    }
    func running() { core.start(); driver.complete(.running) }
    func cleanUp() { core.peerClosed(); driver.complete(.stopped); driver.ended(.stopped) }
    deinit { try? original.close(); try? FileManager.default.removeItem(at: url) }
}

@main
private struct Tests {
    static func main() {
        let q = DispatchQueue(label: "helper-tests")
        q.sync { run(q) }
        let t = Trace()
        q.sync {
            VirtualizationDriver.afterCallback(on: q) { t.terminal.append(true) }
            check(t.terminal.isEmpty, "native continuation ran before SDK callback returned")
        }
        q.sync { check(t.terminal == [true], "native continuation was lost") }
        print("Swift helper tests: \(assertions) assertions, \(failures) failures")
        exit(failures == 0 ? 0 : 1)
    }
    static func run(_ q: DispatchQueue) {
        // Every driver completion is manually released. No sleeps, guests or native VM instances.
        do {
            let h = Harness(q); h.core.start()
            check(h.driver.calls == ["start"], "start must reach driver")
            check(h.trace.writes.isEmpty, "readiness before start completion")
            h.driver.complete(.running)
            check(h.trace.writes == [[66,82,86,77,1]], "exact BRVM v1 readiness")
            h.cleanUp()
        }
        do {
            let h = Harness(q); h.running()
            let bytes = frame(80, 1)
            for b in bytes.dropLast() { h.core.receive([b]) }
            check(h.driver.calls == ["start"], "partial frame dispatched")
            h.core.receive([bytes.last!])
            check(h.driver.calls.last == "pause", "P not dispatched")
            check(h.trace.writes.count == 1, "ACK before pause completion")
            h.driver.complete(.paused)
            check(h.trace.writes.last == frame(75, 1), "P reply ID or endian")
            h.core.receive(frame(82, 2)); h.driver.complete(.running)
            check(h.trace.writes.last == frame(75, 2), "R reply")
            h.core.receive(frame(83, 3))
            check(fcntl(h.fd, F_GETFD) >= 0, "ownership closed before stop completion")
            h.driver.complete(.stopped)
            check(h.driver.calls == ["start","pause","resume","stop","dispose"], "lifecycle order")
            check(h.trace.disposedWithOpenFD, "FD closed before VM disposal")
            check(h.trace.writes.last == frame(75, 3), "S reply")
            check(h.trace.repliesWithClosedFD.last == true, "S ACK before FD close")
            check(h.trace.terminal == [true], "only completed S is success")
            h.core.start(); h.core.receive(frame(80, 4))
            check(h.driver.calls.count == 5 && h.trace.terminal.count == 1, "terminal session restarted")
        }
        for prepared in [false, true] {
            let h = Harness(q); h.running()
            if prepared { h.core.receive(frame(71, 1)) }
            // Native state wins after Rust's last exit check; didEnd is deferred.
            h.driver.state = .stopped
            let delayedEnd = h.driver.didEnd
            let id: UInt64 = prepared ? 2 : 1
            h.core.receive(frame(83, id))
            check(h.trace.writes.last == frame(prepared ? 78 : 75, id), "S lost natural-stop classification")
            check(h.driver.calls == ["start", "dispose"], "already-stopped S invoked destructive stop")
            check(h.trace.disposedWithOpenFD, "S unlocked before disposal")
            check(h.trace.repliesWithClosedFD.last == true, "S receipt before ownership close")
            check(h.trace.terminal == [true], "already-stopped S failed")
            let writes = h.trace.writes.count
            delayedEnd?()
            check(h.trace.writes.count == writes && h.trace.terminal == [true], "deferred end repeated terminal receipt")
            check(h.driver.calls == ["start", "dispose"], "deferred end repeated disposal")
        }
        for terminalState in [MachineState.stopped, .failed] {
            let h = Harness(q); h.running()
            h.core.receive(frame(71, 1)) // G only arms guest-stop observation.
            check(h.driver.calls == ["start"], "G must not hard-stop the VM")
            check(h.trace.writes.last == frame(75, 1), "G preparation was not acknowledged")
            check(fcntl(h.fd, F_GETFD) >= 0, "G released a running owner")
            h.driver.ended(terminalState)
            check(h.driver.calls == ["start", "dispose"], "natural guest stop used destructive stop")
            check(h.trace.disposedWithOpenFD, "guest stop unlocked before disposal")
            check(h.trace.terminal == [terminalState == .stopped], "expected guest stop classification")
        }
        for terminalState in [MachineState.stopped, .failed] {
            let h = Harness(q); h.running()
            // Native state changes before its deferred didEnd callback runs.
            h.driver.state = terminalState
            let delayedEnd = h.driver.didEnd
            h.core.receive(frame(71, 1))
            check(h.trace.writes.last == frame(69, 1), "terminal VM acknowledged G")
            check(h.driver.calls == ["start", "dispose"], "terminal G used destructive stop or leaked")
            check(h.trace.disposedWithOpenFD, "terminal G unlocked before disposal")
            check(h.trace.terminal == [false], "terminal-before-G became clean guest poweroff")
            let writes = h.trace.writes.count
            delayedEnd?()
            check(h.trace.terminal == [false] && h.trace.writes.count == writes, "deferred end changed rejected G")
            check(h.driver.calls == ["start", "dispose"], "deferred end repeated disposal")
        }
        do {
            let h = Harness(q); h.running(); h.driver.ended(.stopped)
            check(h.trace.terminal == [false], "unsolicited stop became expected guest poweroff")
        }
        do {
            let h = Harness(q); h.running(); h.core.receive(frame(71, 1))
            h.core.receive(frame(83, 2))
            check(h.driver.calls.last == "stop", "prepared timeout cannot invoke hard fallback")
            h.driver.complete(.stopped)
            check(h.trace.writes.last == frame(75, 2), "fallback S completion missing")
        }
        do {
            let h = Harness(q); h.running(); h.core.receive(frame(71, 1))
            h.core.peerClosed(); h.driver.complete(.stopped)
            check(h.trace.terminal == [false], "parent EOF became clean guest poweroff")
        }
        for bytes in [frame(88,1), frame(80,0), frame(80,2), [UInt8](repeating: 0, count: 10)] {
            let h = Harness(q); h.running(); h.core.receive(bytes)
            check(h.driver.calls.last == "stop", "invalid traffic did not stop")
            h.driver.complete(.stopped)
            check(h.trace.writes.count == 1 && h.trace.terminal == [false], "invalid traffic replied")
        }
        do {
            let h = Harness(q); h.running(); h.core.receive(frame(80,1)); h.driver.complete(.paused)
            h.core.receive(frame(82,1)); h.driver.complete(.stopped)
            check(h.trace.writes.count == 2, "duplicate ID replied")
        }
        do {
            let h = Harness(q); h.core.start(); h.core.receive(frame(80,1))
            check(h.driver.calls == ["start"], "pre-ready input stopped during start callback")
            h.driver.complete(.running)
            check(h.trace.writes.isEmpty && h.driver.calls.last == "stop", "late readiness after invalid input")
            h.driver.complete(.stopped)
        }
        for duringPause in [false, true] {
            let h = Harness(q); h.core.start()
            if duringPause { h.driver.complete(.running); h.core.receive(frame(80,1)) }
            h.core.peerClosed()
            check(h.driver.calls.last == (duringPause ? "pause" : "start"), "disposed during in-flight operation")
            h.driver.complete(duringPause ? .paused : .running)
            check(h.driver.calls.last == "stop", "EOF cleanup not issued after drain")
            h.driver.complete(.stopped)
            check(h.trace.writes.count == (duringPause ? 1 : 0), "late ACK/readiness after EOF")
            check(h.trace.terminal == [false], "EOF reported success")
        }
        do {
            let h = Harness(q); h.running(); h.core.receive(frame(83,1))
            h.driver.ended(.stopped)
            check(h.trace.terminal.isEmpty && fcntl(h.fd,F_GETFD) >= 0, "delegate disposed before stop callback")
            h.driver.complete(.stopped)
            check(h.trace.terminal == [true], "stop completion lost after delegate")
        }
        do {
            let h = Harness(q); h.running(); h.core.receive(frame(83,1)); h.core.receive(frame(80,2))
            h.driver.complete(.stopped)
            check(h.trace.writes.count == 1 && h.trace.terminal == [false], "post-S traffic replied")
        }
        do {
            let h = Harness(q); h.core.start(); h.driver.complete(.failed, false)
            check(h.driver.calls == ["start","dispose"], "error must dispose, never stop unusable VM")
            check(h.trace.writes.isEmpty && h.trace.terminal == [false], "startup error acknowledged")
        }
        do {
            let h = Harness(q); h.running(); h.core.receive(frame(80,1)); h.driver.ended(.failed)
            check(!h.driver.calls.contains("dispose"), "error disposed before callback drained")
            h.driver.complete(.failed, false)
            check(h.trace.writes.last == frame(69,1) && h.trace.terminal == [false], "error command must fail")
        }
        for pause in [true, false] {
            let h = Harness(q); h.running()
            if pause { h.driver.canPause = false } else { h.driver.canResume = false }
            h.core.receive(frame(pause ? 80 : 82,1)); h.driver.complete(.stopped)
            check(h.driver.calls == ["start","stop","dispose"], "capability refusal invoked operation")
            check(h.trace.writes.last == frame(69,1), "valid refused command needs E")
        }
        do {
            let h = Harness(q); h.core.start(); h.driver.complete(.paused)
            check(h.trace.writes.isEmpty && h.driver.calls.last == "stop", "wrong observed start state acknowledged")
            h.driver.complete(.stopped)
        }
        do {
            let h = Harness(q); h.trace.outputWorks = false; h.running()
            check(h.driver.calls.last == "stop", "failed/partial output did not stop")
            h.driver.complete(.stopped)
            check(h.trace.writes.count == 1 && h.trace.terminal == [false], "output failure retried")
        }
        for nonterminal in [MachineState.running, .paused, .transitioning] {
            let h = Harness(q); h.running(); h.driver.state = nonterminal; h.driver.canStop = false
            h.core.peerClosed()
            check(h.trace.terminal.isEmpty && !h.driver.calls.contains("dispose") && fcntl(h.fd,F_GETFD) >= 0, "unobserved state released ownership")
            h.core.peerClosed()
            check(!h.driver.calls.contains("stop"), "canStop refusal was retried")
            h.driver.ended(.failed)
            check(h.trace.terminal == [false], "late terminal error not disposed")
        }
        do {
            let h = Harness(q); h.running(); h.core.peerClosed(); h.driver.complete(.running, false)
            h.core.peerClosed(); h.core.receive(frame(83,1))
            check(h.driver.calls.filter { $0 == "stop" }.count == 1, "failed stop repeated")
            check(h.trace.terminal.isEmpty && fcntl(h.fd,F_GETFD) >= 0, "unobserved stop released authority")
            h.driver.ended(.stopped)
            check(h.trace.terminal == [false] && h.trace.writes.count == 1, "late stopped observation replied")
        }
        do {
            let h = Harness(q); h.running(); h.core.receive(frame(83,1))
            h.driver.complete(.failed, false)
            check(h.trace.writes.last == frame(69,1) && h.trace.terminal == [false], "native stop error emitted K")
        }
        do {
            var h: Harness? = Harness(q)
            let driver = h!.driver, fd = h!.fd
            let core = { [weak core = h!.core] in core }
            h!.running(); h = nil
            check(core() != nil && fcntl(fd,F_GETFD) >= 0, "caller drop released a live VM owner")
            driver.ended(.failed)
            check(core() == nil && fcntl(fd,F_GETFD) == -1, "terminal owner leaked after disposal")
        }
        do {
            _ = try VirtualizationDriver(configuration: VZVirtualMachineConfiguration(), queue: q)
            check(false, "invalid native configuration reached VM construction")
        } catch { /* Validation is required before any native VM instance exists. */ }
        do {
            let h = Harness(q); h.running(); h.driver.canStop = false
            h.core.receive(frame(83,1))
            check(h.trace.terminal.isEmpty && fcntl(h.fd,F_GETFD) >= 0, "refused S did not quarantine")
            h.driver.ended(.stopped)
            check(h.trace.writes.last == frame(69,1) && h.trace.terminal == [false], "refused S became success after unrelated stop")
        }
        do {
            var h: Harness? = Harness(q)
            let fd = h!.fd
            h!.running(); h!.core.receive(frame(83,1)); h!.driver.complete(.stopped)
            // Reuse a released FD before FileHandle deallocation; never close a
            // descriptor still owned by the core to fabricate an API exception.
            precondition(dup2(h!.original.fileDescriptor, fd) == fd)
            h = nil
            check(fcntl(fd,F_GETFD) >= 0, "owner deallocation closed a reused FD")
            _ = close(fd)
        }
        check(BridgeVM.successor(UInt64.max) == nil, "ID wrapped")
        check(BridgeVM.successor(255) == 256, "ID successor")
        check(VirtualizationDriver.classify(.stopped) == .stopped, "native stopped mapping")
        check(VirtualizationDriver.classify(.error) == .failed, "native error mapping")
        check(VirtualizationDriver.classify(.stopping) == .transitioning, "native stopping mapping")
    }
}
