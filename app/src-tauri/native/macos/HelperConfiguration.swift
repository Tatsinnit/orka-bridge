import Foundation
import Virtualization
import Darwin

enum HelperConfigurationError: Error { case invalidInput, system }

// Parse borrows every FD. Adoption starts only after all four capabilities pass.
struct HelperArguments {
    let configFD: Int32
    let descriptors: InheritedDescriptors

    static func parse(_ args: [String]) throws -> HelperArguments {
        guard args.count == 8, args[0] == "--config-fd", !args[1].isEmpty,
              args[1].utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }),
              let fd = Int32(args[1]), fd >= 3 else { throw HelperConfigurationError.invalidInput }
        let descriptors = try InheritedDescriptors.parse(Array(args.dropFirst(2)))
        let flags = fcntl(fd, F_GETFL)
        var info = stat()
        guard ![descriptors.supervisor, descriptors.guest, descriptors.ownership].contains(fd),
              flags >= 0, flags & O_ACCMODE != O_WRONLY,
              fstat(fd, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG) else {
            throw HelperConfigurationError.invalidInput
        }
        return HelperArguments(configFD: fd, descriptors: descriptors)
    }
}

// Ephemeral native-only input, not a second installation record or image verifier.
struct HelperConfiguration: Decodable {
    private let version: Int
    private let memoryBytes: UInt64
    private let installationPath: String
    private let osDiskPath: String
    private let stateDiskPath: String
    private let machineId: [UInt8]
    private let mac: [UInt8]
    private let createEFI: Bool

    static func read(owningFD fd: Int32) throws -> HelperConfiguration {
        let owner = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        defer { try? owner.close() }
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG),
              info.st_size > 0, info.st_size <= 65536 else { throw HelperConfigurationError.invalidInput }
        var bytes = [UInt8](repeating: 0, count: Int(info.st_size) + 1)
        var count = 0
        while count < bytes.count {
            let n = bytes.withUnsafeMutableBytes {
                pread(fd, $0.baseAddress!.advanced(by: count), $0.count - count, off_t(count))
            }
            if n < 0 && errno == EINTR { continue }
            guard n >= 0 else { throw HelperConfigurationError.system }
            if n == 0 { break }
            count += n
        }
        guard count == Int(info.st_size) else { throw HelperConfigurationError.invalidInput }
        let data = Data(bytes.prefix(count))
        let keys: Set<String> = ["version", "memoryBytes", "installationPath", "osDiskPath", "stateDiskPath",
                                 "machineId", "mac", "createEFI"]
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(object.keys) == keys else { throw HelperConfigurationError.invalidInput }
        let config = try JSONDecoder().decode(Self.self, from: data)
        guard config.version == 1,
              (1...16).contains(config.memoryBytes >> 30), config.memoryBytes % (1 << 20) == 0,
              (1...4096).contains(config.machineId.count),
              VZGenericMachineIdentifier(dataRepresentation: Data(config.machineId)) != nil,
              config.mac.count == 6, config.mac[0] & 3 == 2,
              [config.installationPath, config.osDiskPath, config.stateDiskPath].allSatisfy({
                  $0.hasPrefix("/") && !$0.utf8.contains(0)
              }) else { throw HelperConfigurationError.invalidInput }
        return config
    }

    func makeConfiguration(guest: FileHandle, diagnostic: FileHandle,
                           queue: DispatchQueue) throws -> VZVirtualMachineConfiguration {
        dispatchPrecondition(condition: .onQueue(queue))
        var directory = stat()
        guard lstat(installationPath, &directory) == 0,
              directory.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR) else {
            throw HelperConfigurationError.invalidInput
        }
        let os = try Self.regular(osDiskPath), state = try Self.regular(stateDiskPath)
        guard os.st_dev != state.st_dev || os.st_ino != state.st_ino else {
            throw HelperConfigurationError.invalidInput
        }
        let efi = URL(fileURLWithPath: installationPath).appendingPathComponent("efi.vars")
        let loader = VZEFIBootLoader()
        if createEFI {
            var existing = stat()
            guard lstat(efi.path, &existing) == -1, errno == ENOENT else {
                throw HelperConfigurationError.invalidInput
            }
            loader.variableStore = try VZEFIVariableStore(creatingVariableStoreAt: efi)
        } else {
            let existing = try Self.regular(efi.path)
            guard existing.st_size > 0 else { throw HelperConfigurationError.invalidInput }
            loader.variableStore = VZEFIVariableStore(url: efi)
        }
        let platform = VZGenericPlatformConfiguration()
        guard let machineID = VZGenericMachineIdentifier(dataRepresentation: Data(machineId)) else {
            throw HelperConfigurationError.invalidInput
        }
        platform.machineIdentifier = machineID
        let configuration = VZVirtualMachineConfiguration()
        configuration.cpuCount = 4
        configuration.memorySize = memoryBytes
        configuration.bootLoader = loader
        configuration.platform = platform
        configuration.storageDevices = try [(osDiskPath, "bridge-os"), (stateDiskPath, "bridge-state")].map {
            let attachment = try VZDiskImageStorageDeviceAttachment(url: URL(fileURLWithPath: $0.0), readOnly: false,
                                                                   cachingMode: .cached, synchronizationMode: .full)
            let device = VZVirtioBlockDeviceConfiguration(attachment: attachment)
            device.blockDeviceIdentifier = $0.1
            return device
        }
        configuration.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
        let network = VZVirtioNetworkDeviceConfiguration()
        network.attachment = VZNATNetworkDeviceAttachment()
        guard let address = VZMACAddress(string: mac.map { String(format: "%02x", $0) }.joined(separator: ":")) else {
            throw HelperConfigurationError.invalidInput
        }
        network.macAddress = address
        configuration.networkDevices = [network]
        let control = VZVirtioConsolePortConfiguration()
        control.name = "ai.orka.bridge.control"
        control.isConsole = false
        control.attachment = VZFileHandleSerialPortAttachment(fileHandleForReading: guest, fileHandleForWriting: guest)
        let console = VZVirtioConsoleDeviceConfiguration()
        console.ports[0] = control
        configuration.consoleDevices = [console]
        let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
        serial.attachment = VZFileHandleSerialPortAttachment(fileHandleForReading: nil, fileHandleForWriting: diagnostic)
        configuration.serialPorts = [serial]
        return configuration
    }

    private static func regular(_ path: String) throws -> stat {
        var info = stat()
        guard lstat(path, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG) else {
            throw HelperConfigurationError.invalidInput
        }
        return info
    }
}
