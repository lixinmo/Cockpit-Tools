//! Native CT-CONNECT-RETRY-001 classification. The retry loop lives below the
//! request sender in the narrowly patched hyper-util connection setup boundary.
use std::{error::Error, fmt, io, net::SocketAddr};

pub(crate) fn configure(
    builder: reqwest::ClientBuilder,
    base_url: &str,
    websocket: bool,
) -> reqwest::ClientBuilder {
    let enabled = !websocket
        && reqwest::Url::parse(base_url).is_ok_and(|url| {
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case("chatgpt.com"))
        });
    if enabled {
        // Match upstream Native OAuth's TLS backend and system trust store.
        // macOS native-tls erases the typed OS failure behind its Error, making
        // a closed handshake indistinguishable from permanent TLS failures.
        builder
            .use_rustls_tls()
            .cockpit_connect_retry(retryable)
            .dns_resolver(std::sync::Arc::new(SystemResolver))
    } else {
        builder
    }
}

#[derive(Debug)]
pub(crate) struct DnsFailure {
    code: i32,
    temporary: bool,
    not_found: bool,
    source: Option<io::Error>,
}

impl fmt::Display for DnsFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "system DNS resolution failed ({})", self.code)
    }
}
impl Error for DnsFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|error| error as &(dyn Error + 'static))
    }
}

pub(crate) fn retryable(error: &(dyn Error + 'static)) -> bool {
    let mut pending = vec![error];
    let mut seen: Vec<*const (dyn Error + 'static)> = Vec::new();
    let mut transient = false;
    // Concrete causes, never wrapper names or Display strings, establish
    // eligibility. Certificate/TLS and NXDOMAIN exclusions override timeouts.
    while let Some(error) = pending.pop() {
        let pointer = error as *const (dyn Error + 'static);
        if seen.iter().any(|previous| std::ptr::eq(*previous, pointer)) {
            continue;
        }
        if seen.len() >= 256 {
            return false;
        }
        seen.push(pointer);
        if error.is::<rustls::Error>() || error.is::<native_tls::Error>() {
            return false;
        }
        if let Some(dns) = error.downcast_ref::<DnsFailure>() {
            if dns.not_found {
                return false;
            }
            transient |= dns.temporary;
        }
        if let Some(io) = error.downcast_ref::<io::Error>() {
            // io::Error::source() delegates to its payload's source(), skipping
            // the payload itself. Inspect get_ref() or a nested certificate /
            // NXDOMAIN error could be hidden by an outer timeout kind.
            if let Some(cause) = io.get_ref() {
                pending.push(cause);
            }
            transient |= matches!(
                io.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::HostUnreachable
                    | io::ErrorKind::NetworkUnreachable
            );
        }
        transient |=
            reqwest::cockpit_is_connect_timeout(error) || error.is::<tokio::time::error::Elapsed>();
        if let Some(cause) = error.source() {
            pending.push(cause);
        }
    }
    transient
}

/// std's getaddrinfo adapter erases EAI_AGAIN/NXDOMAIN into text. Keep the
/// original system resolver and its ordered addresses, but retain typed causes.
#[derive(Debug)]
pub(crate) struct SystemResolver;

impl reqwest::dns::Resolve for SystemResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let name = name.as_str().to_owned();
        Box::pin(async move {
            let worker = tokio::task::spawn_blocking(move || resolve_system(&name));
            // Cancel queued work when the enclosing request is dropped. An OS
            // getaddrinfo already running cannot be interrupted; its result is
            // discarded and cannot trigger another attempt after cancellation.
            struct AbortQueued(tokio::task::AbortHandle);
            impl Drop for AbortQueued {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _queued = AbortQueued(worker.abort_handle());
            let addresses = worker
                .await
                .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)??;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(unix)]
fn resolve_system(name: &str) -> Result<Vec<SocketAddr>, Box<dyn Error + Send + Sync>> {
    use std::{
        ffi::CString,
        net::{Ipv4Addr, Ipv6Addr, SocketAddrV6},
        ptr,
    };
    let name = CString::new(name)?;
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;
    let mut result = ptr::null_mut();
    let code = unsafe { libc::getaddrinfo(name.as_ptr(), ptr::null(), &hints, &mut result) };
    if code != 0 {
        return Err(Box::new(DnsFailure {
            code,
            temporary: code == libc::EAI_AGAIN,
            not_found: code == libc::EAI_NONAME,
            source: (code == libc::EAI_SYSTEM).then(io::Error::last_os_error),
        }));
    }
    struct Addresses(*mut libc::addrinfo);
    impl Drop for Addresses {
        fn drop(&mut self) {
            unsafe { libc::freeaddrinfo(self.0) }
        }
    }
    let addresses = Addresses(result);
    let mut cursor = addresses.0;
    let mut resolved = Vec::new();
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        if !entry.ai_addr.is_null() {
            match entry.ai_family {
                libc::AF_INET
                    if entry.ai_addrlen as usize >= std::mem::size_of::<libc::sockaddr_in>() =>
                {
                    let address = unsafe { &*entry.ai_addr.cast::<libc::sockaddr_in>() };
                    resolved.push(SocketAddr::from((
                        Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()),
                        0,
                    )));
                }
                libc::AF_INET6
                    if entry.ai_addrlen as usize >= std::mem::size_of::<libc::sockaddr_in6>() =>
                {
                    let address = unsafe { &*entry.ai_addr.cast::<libc::sockaddr_in6>() };
                    resolved.push(SocketAddr::V6(SocketAddrV6::new(
                        Ipv6Addr::from(address.sin6_addr.s6_addr),
                        0,
                        address.sin6_flowinfo,
                        address.sin6_scope_id,
                    )));
                }
                _ => {}
            }
        }
        cursor = entry.ai_next;
    }
    if resolved.is_empty() {
        return Err(Box::new(DnsFailure {
            code: libc::EAI_NONAME,
            temporary: false,
            not_found: true,
            source: None,
        }));
    }
    Ok(resolved)
}

// The requested build targets this Mac. Other platforms retain their original
// resolver until their OS-specific DNS classifications have been validated.
#[cfg(not(unix))]
fn resolve_system(name: &str) -> Result<Vec<SocketAddr>, Box<dyn Error + Send + Sync>> {
    use std::net::ToSocketAddrs;
    Ok((name, 0).to_socket_addrs()?.collect())
}

#[cfg(test)]
#[path = "codex_native_connect_retry_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "codex_native_connect_retry_integration_tests.rs"]
mod integration_tests;

#[cfg(test)]
#[path = "codex_native_connect_retry_h2_tests.rs"]
mod h2_tests;
