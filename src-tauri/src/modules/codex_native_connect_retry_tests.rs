use super::*;

#[test]
fn native_setup_retry_accepts_only_concrete_transient_causes() {
    for kind in [
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::ConnectionRefused,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::TimedOut,
        io::ErrorKind::HostUnreachable,
        io::ErrorKind::NetworkUnreachable,
    ] {
        assert!(retryable(&io::Error::from(kind)), "{kind:?}");
    }
    for kind in [
        io::ErrorKind::Other,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::InvalidData,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::NotFound,
        io::ErrorKind::WouldBlock,
        io::ErrorKind::Interrupted,
    ] {
        assert!(
            !retryable(&io::Error::new(kind, "EOF connection reset timeout")),
            "{kind:?}"
        );
    }
}

#[test]
fn native_setup_retry_dns_nxdomain_wins_over_timeout() {
    let temporary = DnsFailure {
        code: 1,
        temporary: true,
        not_found: false,
        source: None,
    };
    assert!(retryable(&temporary));
    let permanent = DnsFailure {
        code: 2,
        temporary: true,
        not_found: true,
        source: Some(io::Error::from(io::ErrorKind::TimedOut)),
    };
    assert!(!retryable(&permanent));
    let wrapped = io::Error::new(io::ErrorKind::TimedOut, permanent);
    assert!(!retryable(&wrapped));
    let system = DnsFailure {
        code: 3,
        temporary: false,
        not_found: false,
        source: Some(io::Error::from(io::ErrorKind::ConnectionReset)),
    };
    assert!(retryable(&system));
}

#[test]
fn native_setup_retry_permanent_tls_wins_over_timeout() {
    let tls = rustls::Error::InvalidCertificate(rustls::CertificateError::Expired);
    assert!(!retryable(&tls));
    assert!(!retryable(&io::Error::new(io::ErrorKind::TimedOut, tls)));
}

#[test]
fn native_setup_retry_keeps_system_localhost_resolution() {
    let addresses = resolve_system("localhost").unwrap();
    assert!(!addresses.is_empty());
    assert!(addresses
        .iter()
        .all(|address| address.ip().is_loopback() && address.port() == 0));
    assert!(resolve_system("bad\0hostname").is_err());
}
