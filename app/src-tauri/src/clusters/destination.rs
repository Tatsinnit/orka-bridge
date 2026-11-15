//! Configured transport identity, not server or workload attestation. Never hash credentials.
use super::{AccessOutcome, Check};
use sha2::{Digest, Sha256};

pub(super) fn fingerprint(config: &kube::Config) -> Result<String, AccessOutcome> {
    let failed = || Check::Configuration.unknown();
    // All application callers use from_custom_kubeconfig, which resolves CA
    // files into root_cert. Never reopen paths on the async forwarding worker.
    if config.root_cert_file.is_some() {
        return Err(failed());
    }
    let bytes = serde_json::to_vec(&(
        config.cluster_url.to_string(),
        &config.root_cert,
        // Preserve the empty file slot in already-saved fingerprints.
        Option::<Vec<u8>>::None,
        &config.tls_server_name,
        config.accept_invalid_certs,
    ))
    .map_err(|_| failed())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn valid_identity(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unresolved_trust_files_are_refused_instead_of_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca");
        std::fs::write(&path, "synthetic-ca").unwrap();
        let mut config = kube::Config::new("https://cluster.invalid/".parse().unwrap());
        config.root_cert_file = Some(path);
        assert!(fingerprint(&config).is_err());
    }
    #[tokio::test]
    async fn kubeconfig_ca_file_contents_are_resolved_before_fingerprinting() {
        use base64::Engine;
        let ca = || {
            let cert = rcgen::generate_simple_self_signed(vec!["fixture.invalid".into()]).unwrap();
            format!(
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
                base64::engine::general_purpose::STANDARD.encode(cert.cert.der())
            )
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, ca()).unwrap();
        let mut config: kube::config::Kubeconfig = serde_json::from_value(serde_json::json!({
            "apiVersion":"v1","kind":"Config","clusters":[{"name":"cluster","cluster":{"server":"https://cluster.invalid/root","certificate-authority":path}}],
            "contexts":[{"name":"team","context":{"cluster":"cluster","user":"creator"}}],"users":[{"name":"creator","user":{"token":"fixture"}}]
        })).unwrap();
        let resolved = super::super::credentials::selected_config(&config, "team", "fixture")
            .await
            .unwrap();
        assert!(resolved.root_cert_file.is_none());
        let first = fingerprint(&resolved).unwrap();
        let second_pem = ca();
        std::fs::write(&path, &second_pem).unwrap();
        let resolved = super::super::credentials::selected_config(&config, "team", "fixture")
            .await
            .unwrap();
        let second = fingerprint(&resolved).unwrap();
        assert_ne!(first, second);
        let cluster = config.clusters[0].cluster.as_mut().unwrap();
        cluster.certificate_authority = None;
        cluster.certificate_authority_data =
            Some(base64::engine::general_purpose::STANDARD.encode(second_pem));
        let inline = super::super::credentials::selected_config(&config, "team", "other-auth")
            .await
            .unwrap();
        assert_eq!(fingerprint(&inline).unwrap(), second);
    }
    #[test]
    fn identity_tracks_destination_and_trust_but_not_authentication() {
        let mut config = kube::Config::new("https://cluster.invalid/root".parse().unwrap());
        let identity = fingerprint(&config).unwrap();
        config.auth_info.token = Some("synthetic-not-an-identity".into());
        assert_eq!(fingerprint(&config).unwrap(), identity);
        for changed in [
            "https://other.invalid/root",
            "https://cluster.invalid/other",
        ] {
            let mut other = config.clone();
            other.cluster_url = changed.parse().unwrap();
            assert_ne!(fingerprint(&other).unwrap(), identity);
        }
        config.root_cert = Some(vec![vec![1, 2, 3]]);
        assert_ne!(fingerprint(&config).unwrap(), identity);
        config.root_cert = Some(vec![vec![4, 5, 6]]);
        assert_ne!(fingerprint(&config).unwrap(), identity);
    }
}
