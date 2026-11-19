import Foundation
import Virtualization
import Darwin

private var failures = 0
private var assertions = 0
private func check(_ value: @autoclosure () -> Bool, _ label: String, line: Int = #line) {
    assertions += 1
    if !value() { failures += 1; print("FAIL \(line): \(label)") }
}
private func rejects(_ label: String, _ operation: () throws -> Void) {
    do { try operation(); check(false, label) } catch { check(true, label) }
}
private func pair() -> [Int32] {
    var fds: [Int32] = [-1, -1]
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
    return fds
}

// Synthetic owned files only. No configuration test constructs a VZ VM.
private final class Fixture {
    let root: URL
    let owner: FileHandle
    let control = pair(), guest = pair()
    let machineID = VZGenericMachineIdentifier().dataRepresentation
    init() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
                                              attributes: [.posixPermissions: 0o700])
        for name in ["owner", "os.disk", "state.disk"] {
            try Data().write(to: root.appendingPathComponent(name))
        }
        owner = try FileHandle(forUpdating: root.appendingPathComponent("owner"))
        for name in ["os.disk", "state.disk"] {
            let disk = try FileHandle(forUpdating: root.appendingPathComponent(name))
            try disk.truncate(atOffset: 1024 * 1024)
            try disk.close()
        }
    }
    var json: [String: Any] {
        ["version": 1, "memoryBytes": 6 * 1024 * 1024 * 1024, "installationPath": root.path,
         "osDiskPath": root.appendingPathComponent("os.disk").path,
         "stateDiskPath": root.appendingPathComponent("state.disk").path,
         "machineId": Array(machineID), "mac": [2, 1, 2, 3, 4, 5], "createEFI": true]
    }
    var args: [String] {
        ["--supervisor-fd", String(control[0]), "--guest-fd", String(guest[0]),
         "--ownership-fd", String(owner.fileDescriptor)]
    }
    func configFD(_ data: Data) throws -> Int32 {
        let path = root.appendingPathComponent(UUID().uuidString)
        try data.write(to: path)
        let fd = open(path.path, O_RDONLY | O_CLOEXEC)
        precondition(fd >= 3)
        precondition(unlink(path.path) == 0)
        return fd
    }
    func read(_ object: [String: Any]) throws -> HelperConfiguration {
        let fd = try configFD(JSONSerialization.data(withJSONObject: object))
        defer { check(fcntl(fd, F_GETFD) == -1, "adopted config FD not closed") }
        return try HelperConfiguration.read(owningFD: fd)
    }
    deinit {
        try? owner.close()
        for fd in control + guest { _ = close(fd) }
        try? FileManager.default.removeItem(at: root)
    }
}

@main
private struct ConfigurationTests {
    static func main() {
        do {
            let f = try Fixture()
            try arguments(f)
            try decoding(f)
            try construction(f)
        } catch { check(false, "unexpected fixture/construction failure") }
        print("Swift helper configuration: \(assertions) assertions, \(failures) failures")
        exit(failures == 0 ? 0 : 1)
    }

    private static func arguments(_ f: Fixture) throws {
        let fd = try f.configFD(JSONSerialization.data(withJSONObject: f.json))
        defer { _ = close(fd) }
        let good = ["--config-fd", String(fd)] + f.args
        let parsed = try HelperArguments.parse(good)
        check(parsed.configFD == fd && parsed.descriptors.guest == f.guest[0], "selector not separated")
        var invalid: [[String]] = [f.args, ["--config-fd"]]
        for number in ["-1", "+3", "2147483648", "2", String(f.owner.fileDescriptor),
                       String(f.control[0]), String(f.guest[1])] {
            invalid.append(["--config-fd", number] + f.args)
        }
        invalid.append(good + ["--config-fd", String(fd)])
        for args in invalid {
            rejects("invalid selector accepted") { _ = try HelperArguments.parse(args) }
            check(([fd, f.owner.fileDescriptor] + f.control + f.guest).allSatisfy {
                fcntl($0, F_GETFD) >= 0
            }, "parser closed a borrowed FD")
        }
        let path = f.root.appendingPathComponent("unreadable")
        let writeOnly = open(path.path, O_CREAT | O_WRONLY | O_CLOEXEC, 0o600)
        precondition(writeOnly >= 3)
        defer { _ = close(writeOnly) }
        rejects("write-only config FD accepted") {
            _ = try HelperArguments.parse(["--config-fd", String(writeOnly)] + f.args)
        }
        check(fcntl(writeOnly, F_GETFD) >= 0, "unreadable borrowed FD closed")
    }

    private static func decoding(_ f: Fixture) throws {
        _ = try f.read(f.json)
        let mutations: [(String, Any)] = [
            ("extra", true), ("memoryBytes", 0), ("memoryBytes", 1 << 40),
            ("memoryBytes", (6 << 30) + 1), ("memoryBytes", -1), ("version", 2), ("version", true), ("version", "1"),
            ("createEFI", 1), ("machineId", [] as [Int]), ("machineId", [1, 2, 3]),
            ("machineId", [Int](repeating: 0, count: 4097)), ("machineId", [-1]),
            ("machineId", [256]), ("mac", [2, 1]), ("mac", [3, 1, 2, 3, 4, 5]),
            ("mac", [0, 1, 2, 3, 4, 5]), ("mac", [2, 1, 2, 3, 4, 256]),
            ("installationPath", "relative"), ("osDiskPath", "relative"),
            ("stateDiskPath", "relative"), ("installationPath", "/bad\0path")]
        for (key, value) in mutations {
            var object = f.json; object[key] = value
            rejects("bad configuration value accepted") { _ = try f.read(object) }
        }
        var missing = f.json; missing.removeValue(forKey: "mac")
        rejects("missing field accepted") { _ = try f.read(missing) }
        for data in [Data(), Data("{".utf8), Data("[]".utf8), Data(repeating: 32, count: 65537)] {
            let fd = try f.configFD(data)
            rejects("bad config bytes accepted") { _ = try HelperConfiguration.read(owningFD: fd) }
            check(fcntl(fd, F_GETFD) == -1, "invalid config leaked adopted FD")
        }
        let data = try JSONSerialization.data(withJSONObject: f.json)
        let fd = try f.configFD(data)
        _ = lseek(fd, 0, SEEK_END)
        _ = try HelperConfiguration.read(owningFD: fd)
        check(fcntl(fd, F_GETFD) == -1, "nonzero offset leaked FD")
    }

    private static func construction(_ f: Fixture) throws {
        let q = DispatchQueue(label: "configuration-test")
        let guest = FileHandle(fileDescriptor: f.guest[0], closeOnDealloc: false)
        let diagnostic = try FileHandle(forWritingTo: URL(fileURLWithPath: "/dev/null"))
        defer { try? diagnostic.close() }
        let first = try f.read(f.json)
        let config = try q.sync { try first.makeConfiguration(guest: guest, diagnostic: diagnostic, queue: q) }
        check(config.cpuCount == 4 && config.memorySize == 6 * 1024 * 1024 * 1024, "guest sizing")
        check(config.bootLoader is VZEFIBootLoader, "not UEFI")
        let platform = config.platform as? VZGenericPlatformConfiguration
        check(platform?.machineIdentifier.dataRepresentation == f.machineID, "identity replaced")
        check(config.storageDevices.count == 2, "disk count")
        let disks = config.storageDevices.compactMap { $0 as? VZVirtioBlockDeviceConfiguration }
        check(disks.map { $0.blockDeviceIdentifier } == ["bridge-os", "bridge-state"], "disk identity/order")
        check(disks.allSatisfy { ($0.attachment as? VZDiskImageStorageDeviceAttachment)?.isReadOnly == false }, "readonly disk")
        let attachments = disks.compactMap { $0.attachment as? VZDiskImageStorageDeviceAttachment }
        check(attachments.map(\.cachingMode) == [.cached, .cached], "OS/state host caching: \(attachments.map { $0.cachingMode.rawValue })")
        check(attachments.map(\.synchronizationMode) == [.full, .full], "OS/state synchronization: \(attachments.map { $0.synchronizationMode.rawValue })")
        check(config.entropyDevices.count == 1 && config.entropyDevices[0] is VZVirtioEntropyDeviceConfiguration, "entropy")
        check(config.networkDevices.count == 1, "network count")
        check(config.networkDevices.first?.attachment is VZNATNetworkDeviceAttachment, "not NAT")
        check(config.networkDevices.first?.macAddress.string == "02:01:02:03:04:05", "MAC replaced")
        let consoles = config.consoleDevices.compactMap { $0 as? VZVirtioConsoleDeviceConfiguration }
        check(consoles.count == 1, "control console device count")
        let port = consoles.first?.ports[0]
        check(port?.name == "ai.orka.bridge.control" && port?.isConsole == false, "private port zero")
        let channel = port?.attachment as? VZFileHandleSerialPortAttachment
        check(channel?.fileHandleForReading?.fileDescriptor == f.guest[0], "control read FD")
        check(channel?.fileHandleForWriting?.fileDescriptor == f.guest[0], "control write FD")
        check(config.serialPorts.count == 1 && config.serialPorts[0] is VZVirtioConsoleDeviceSerialPortConfiguration, "separate serial")
        let diag = config.serialPorts.first?.attachment as? VZFileHandleSerialPortAttachment
        check(diag?.fileHandleForReading == nil, "diagnostic input enabled")
        check(diag?.fileHandleForWriting?.fileDescriptor == diagnostic.fileDescriptor, "diagnostic sink changed")
        check(config.directorySharingDevices.isEmpty, "host share enabled")
        let efi = f.root.appendingPathComponent("efi.vars")
        let before = try Data(contentsOf: efi)
        rejects("EFI create replaced existing file") {
            _ = try q.sync { try first.makeConfiguration(guest: guest, diagnostic: diagnostic, queue: q) }
        }
        var reopen = f.json; reopen["createEFI"] = false
        let second = try f.read(reopen)
        let again = try q.sync { try second.makeConfiguration(guest: guest, diagnostic: diagnostic, queue: q) }
        check((again.platform as? VZGenericPlatformConfiguration)?.machineIdentifier.dataRepresentation == f.machineID, "reopen identity changed")
        check(again.networkDevices.first?.macAddress.string == "02:01:02:03:04:05", "reopen MAC changed")
        let after = try Data(contentsOf: efi)
        check(before == after, "EFI changed during configuration reopen")
        for value in [f.root.appendingPathComponent("absent").path, f.json["osDiskPath"] as! String] {
            var bad = reopen; bad["stateDiskPath"] = value
            rejects("missing/aliased disk accepted") {
                let c = try f.read(bad)
                _ = try q.sync { try c.makeConfiguration(guest: guest, diagnostic: diagnostic, queue: q) }
            }
        }
        let fresh = try Fixture()
        var absent = fresh.json; absent["createEFI"] = false
        rejects("missing EFI reopened") {
            let c = try fresh.read(absent)
            _ = try q.sync { try c.makeConfiguration(guest: guest, diagnostic: diagnostic, queue: q) }
        }
        check(fcntl(f.guest[0], F_GETFD) >= 0, "builder closed borrowed guest")
    }
}
