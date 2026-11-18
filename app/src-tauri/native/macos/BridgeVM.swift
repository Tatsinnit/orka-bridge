import Foundation

enum MachineState { case stopped, running, paused, transitioning, failed }

// The native implementation owns the sole VM/configuration references. Success
// means the operation completed; the core independently checks observed state.
// Deliver SDK completions/delegate events only after their native stacks unwind.
protocol MachineDriver: AnyObject {
    var state: MachineState { get }
    var canStart: Bool { get }
    var canPause: Bool { get }
    var canResume: Bool { get }
    var canStop: Bool { get }
    var didEnd: (() -> Void)? { get set }
    func start(_ completion: @escaping (Bool) -> Void)
    func pause(_ completion: @escaping (Bool) -> Void)
    func resume(_ completion: @escaping (Bool) -> Void)
    func stop(_ completion: @escaping (Bool) -> Void)
    func dispose()
}

// Private v1 supervisor policy, not an executable/bootstrap or guest handoff.
// All entries and callbacks use the driver's serial queue. The output sink MUST
// be nonblocking and return true only after a full write, never queued acceptance.
final class BridgeVM {
    private enum Operation: Equatable { case start, pause(UInt64), resume(UInt64), stop }
    private let driver: MachineDriver
    private let owner: FileHandle
    private let queue: DispatchQueue
    private let output: ([UInt8]) -> Bool
    private let terminal: (Bool) -> Void
    private var retained: BridgeVM?
    private var pending: Operation?
    private var started = false
    private var closing = false
    private var done = false
    private var writable = true
    // Supervisor success includes an observed S; only G + natural stop proves
    // guest-initiated termination. Rust records those outcomes separately.
    private var supervisorSucceeded = false
    private var expectingGuestStop = false
    private var cleanupAttempted = false
    private var buffer: [UInt8] = []
    private var expectedID: UInt64 = 1
    private var reply: (UInt8, UInt64)?

    // Transfers the inherited ownership FD. No path reopening or second lock.
    init(driver: MachineDriver, ownershipFD: Int32, queue: DispatchQueue,
         output: @escaping ([UInt8]) -> Bool, terminal: @escaping (Bool) -> Void) {
        self.driver = driver
        owner = FileHandle(fileDescriptor: ownershipFD, closeOnDealloc: true)
        self.queue = queue
        self.output = output
        self.terminal = terminal
    }

    func start() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard !started && !done && !closing else { return }
        started = true
        // A dropped caller cannot release ownership while VZ still runs or while
        // an API callback is in flight. Failed observation deliberately retains us.
        retained = self
        driver.didEnd = { [weak self] in self?.ended() }
        guard driver.canStart else { closing = true; cleanup(); return }
        pending = .start
        driver.start { [weak self] ok in self?.completed(.start, ok) }
    }

    func receive(_ bytes: [UInt8]) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard !done && !bytes.isEmpty else { return }
        guard started && !closing && pending == nil && bytes.count <= 9 - buffer.count else {
            invalidate(); return
        }
        buffer.append(contentsOf: bytes)
        guard buffer.count == 9 else { return }
        let opcode = buffer[0]
        let id = buffer.dropFirst().enumerated().reduce(UInt64(0)) {
            $0 | (UInt64($1.element) << ($1.offset * 8))
        }
        buffer.removeAll(keepingCapacity: true)
        guard [UInt8(71),80,82,83].contains(opcode), id == expectedID,
              !expectingGuestStop || opcode == 83,
              let next = Self.successor(id) else { invalidate(); return }
        expectedID = next
        if opcode == 71 {
            // A deferred delegate may follow the native terminal state. G must
            // not reclassify that already-unsolicited exit as guest-requested.
            guard driver.state != .stopped && driver.state != .failed else {
                closing = true; reply = (69, id); cleanup(); return
            }
            // Arm before acknowledging: the guest may power off immediately
            // after the host receives this preparation receipt.
            expectingGuestStop = true
            if !output(Self.frame(75, id)) { invalidate() }
        } else if opcode == 83 {
            // Native state can become stopped before its deferred didEnd and
            // after Rust's last exit check. Preserve that natural-stop receipt
            // only when G was armed; ordinary S retains its existing K reply.
            let receipt: UInt8 = expectingGuestStop && driver.state == .stopped ? 78 : 75
            closing = true; supervisorSucceeded = true; reply = (receipt, id); cleanup()
        } else if opcode == 80 && driver.canPause {
            let operation = Operation.pause(id)
            pending = operation
            driver.pause { [weak self] ok in self?.completed(operation, ok) }
        } else if opcode == 82 && driver.canResume {
            let operation = Operation.resume(id)
            pending = operation
            driver.resume { [weak self] ok in self?.completed(operation, ok) }
        } else {
            closing = true; reply = (69, id); cleanup()
        }
    }

    func peerClosed() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard !done else { return }
        invalidate()
    }

    static func successor(_ id: UInt64) -> UInt64? {
        let (next, overflow) = id.addingReportingOverflow(1)
        return overflow ? nil : next
    }

    private func invalidate() {
        closing = true; writable = false; supervisorSucceeded = false; reply = nil
        buffer.removeAll(keepingCapacity: true)
        cleanup()
    }

    private func completed(_ operation: Operation, _ ok: Bool) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard !done && pending == operation else { return }
        pending = nil
        if operation == .stop {
            if !ok || driver.state != .stopped {
                supervisorSucceeded = false
                if let id = reply?.1 { reply = (69, id) }
            }
            finishIfTerminal()
            return
        }
        if closing { cleanup(); return }
        let expected: MachineState
        let id: UInt64?
        switch operation {
        case .start: expected = .running; id = nil
        case .pause(let value): expected = .paused; id = value
        case .resume(let value): expected = .running; id = value
        case .stop: return
        }
        guard ok && driver.state == expected else {
            closing = true
            if let id = id { reply = (69, id) }
            cleanup()
            return
        }
        let bytes = id.map { Self.frame(75, $0) } ?? [66,82,86,77,1]
        if !output(bytes) { invalidate() }
    }

    private func ended() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard !done else { return }
        // A stop delegate can precede its completion. Never dispose under that
        // callback or convert a successful requested stop into an unexpected one.
        if pending == .stop { return }
        if !closing {
            closing = true
            supervisorSucceeded = expectingGuestStop && pending == nil && driver.state == .stopped
            switch pending {
            case .pause(let id), .resume(let id): reply = (69, id)
            default: break
            }
        }
        cleanup()
    }

    private func cleanup() {
        guard !done && pending == nil else { return }
        if driver.state == .stopped || driver.state == .failed {
            finishIfTerminal(); return
        }
        guard !cleanupAttempted else { return }
        cleanupAttempted = true
        guard driver.canStop else {
            // A refused S stays failed even if a later independent event stops
            // the VM. Terminal observation permits disposal, not retroactive ACK.
            supervisorSucceeded = false
            if let id = reply?.1 { reply = (69, id) }
            return
        }
        pending = .stop
        driver.stop { [weak self] ok in self?.completed(.stop, ok) }
    }

    private func finishIfTerminal() {
        guard !done && pending == nil else { return }
        guard driver.state == .stopped || driver.state == .failed else { return }
        if driver.state == .failed {
            supervisorSucceeded = false
            if let id = reply?.1 { reply = (69, id) }
        }
        done = true
        driver.didEnd = nil
        driver.dispose()
        do { try owner.close() } catch {
            supervisorSucceeded = false
            if let id = reply?.1 { reply = (69, id) }
        }
        if writable, let (opcode, id) = reply {
            if !output(Self.frame(opcode, id)) { supervisorSucceeded = false }
        }
        terminal(supervisorSucceeded)
        retained = nil
    }

    private static func frame(_ opcode: UInt8, _ id: UInt64) -> [UInt8] {
        [opcode] + (0..<8).map { UInt8(truncatingIfNeeded: id >> ($0 * 8)) }
    }
}
