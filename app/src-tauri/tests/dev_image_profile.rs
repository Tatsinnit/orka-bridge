use std::fs;

fn contains(binary: &[u8], needle: &[u8]) -> bool {
    binary.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn actual_app_binary_contains_the_local_image_selector_only_in_debug() {
    let binary = fs::read(env!("CARGO_BIN_EXE_orka-bridge")).unwrap();
    // Cargo supplies the actual app target. Check its executable format, not
    // an unrelated message whose wording can change without changing this gate.
    #[cfg(windows)]
    assert!(binary.starts_with(b"MZ"));
    #[cfg(target_os = "macos")]
    assert!(binary.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]));
    #[cfg(target_os = "linux")]
    assert!(binary.starts_with(b"\x7fELF"));
    assert_eq!(
        contains(&binary, b"BRIDGE_DEV_IMAGE_DIR"),
        cfg!(debug_assertions),
        "The local image selector must exist only in the debug app"
    );
    #[cfg(all(
        not(debug_assertions),
        any(
            all(windows, target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )
    ))]
    for hash in if cfg!(windows) {
        [
            "9d25627b1b5e6bd7952f18f18e8850f531794c8474e7f76aff29affd9a0d8e20",
            "226cfa6e3f7ffc3cde467a3c8e9498aeb1db2aeac68e60c3f27e8f086528850c",
            "4a44487bb3cfdc2be1d7697b8c71c97733d6ab2db62e6e6e8f1c6af9431ffd2e",
            "2a05265fb7e7407e389fc7518ded27f876545cd50d2e14d994b2c9e9cf3fd414",
        ]
    } else {
        [
            "2aec683295a0358a9a5b41b45e45db4ef2776ed9526d75bc7bc0d9d1b7c00c13",
            "3d4379c1002febfb31b081b92805ffd542eca3ae5e9b65f007e20f1d8656b160",
            "27f833e233753a0a601f3a6792dcd73e2f2c7c8bd30e00b837315c71c5996dcb",
            "375394b72b6aeea1e6cc70e00cfba99d6269d0936ece451657449da1899dad93",
        ]
    } {
        let raw: Vec<u8> = (0..64)
            .step_by(2)
            .map(|index| u8::from_str_radix(&hash[index..index + 2], 16).unwrap())
            .collect();
        assert!(
            contains(&binary, hash.as_bytes()) || contains(&binary, &raw),
            "The supported release must carry its reviewed image/runtime pins"
        );
    }
}
