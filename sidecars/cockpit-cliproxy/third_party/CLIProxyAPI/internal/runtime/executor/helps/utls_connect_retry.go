package helps

import (
	"context"
	"crypto/x509"
	"errors"
	"io"
	"net"
	"net/http"
	"time"
)

const (
	// CT-CONNECT-RETRY-001 deliberately keeps this budget fixed and local to
	// connection setup. The initial attempt plus two retries must complete
	// before the caller can hand the error back to credential failover.
	cockpitConnectMaxAttempts = 3
	cockpitConnectBaseDelay   = 200 * time.Millisecond
)

type utlsClientConnection interface {
	RoundTrip(*http.Request) (*http.Response, error)
	Close() error
}

type utlsConnectionFactory func(context.Context, string, string) (utlsClientConnection, error)
type utlsRetryWaiter func(context.Context, time.Duration) error

var errNilUtlsConnection = errors.New("utls: connection setup returned no connection")

func (t *utlsRoundTripper) createConnectionWithRetry(ctx context.Context, host, addr string) (utlsClientConnection, error) {
	connect := t.connectFn
	if connect == nil {
		connect = func(ctx context.Context, host, addr string) (utlsClientConnection, error) {
			connection, errConnect := t.createConnection(ctx, host, addr)
			if connection == nil {
				return nil, errConnect
			}
			return connection, errConnect
		}
	}
	wait := t.waitFn
	if wait == nil {
		wait = waitForUtlsConnectionRetry
	}

	for attempt := 0; attempt < cockpitConnectMaxAttempts; attempt++ {
		if errContext := ctx.Err(); errContext != nil {
			return nil, errContext
		}

		connection, errConnect := connect(ctx, host, addr)
		if errConnect == nil && connection != nil {
			return connection, nil
		}
		if errConnect == nil {
			errConnect = errNilUtlsConnection
		}
		if connection != nil {
			_ = connection.Close()
		}

		if errContext := ctx.Err(); errContext != nil {
			return nil, errContext
		}
		if attempt == cockpitConnectMaxAttempts-1 || !isRetryableUtlsConnectionError(errConnect) {
			return nil, errConnect
		}

		delay := cockpitConnectBaseDelay << attempt
		if errWait := wait(ctx, delay); errWait != nil {
			return nil, errWait
		}
	}

	panic("unreachable uTLS connection retry state")
}

// isRetryableUtlsConnectionError is intentionally a positive whitelist. It
// only sees failures from DNS/proxy/TCP/TLS/HTTP2 connection initialization;
// response errors and stream failures occur after this helper has returned.
func isRetryableUtlsConnectionError(err error) bool {
	if err == nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		return false
	}

	// A negative DNS answer is permanent for this request. Inspect the complete
	// tree so a temporary node cannot hide a joined NXDOMAIN, then allow other
	// concrete causes wrapped by DNSError.UnwrapErr to be classified below.
	dnsNotFound, dnsTransient := classifyUtlsDNSErrors(err)
	if dnsNotFound {
		return false
	}
	if isPermanentUtlsCertificateError(err) {
		return false
	}
	if dnsTransient {
		return true
	}

	if errors.Is(err, io.EOF) ||
		errors.Is(err, io.ErrUnexpectedEOF) ||
		errors.Is(err, io.ErrClosedPipe) ||
		errors.Is(err, net.ErrClosed) {
		return true
	}

	if isRetryableUtlsSyscallError(err) {
		return true
	}

	// A timeout is a specific transient signal. Inspect every wrapped/joined
	// node so a false-valued outer Timeout method cannot hide a true inner one.
	// Merely implementing net.Error, net.OpError, or Temporary is insufficient.
	timedOut := false
	walkUtlsErrorTree(err, func(current error) {
		if timeoutErr, ok := current.(interface{ Timeout() bool }); ok && timeoutErr.Timeout() {
			timedOut = true
		}
	})
	return timedOut
}

func classifyUtlsDNSErrors(err error) (notFound, transient bool) {
	walkUtlsErrorTree(err, func(current error) {
		dnsErr, ok := current.(*net.DNSError)
		if !ok || dnsErr == nil {
			return
		}
		if dnsErr.IsNotFound {
			notFound = true
		}
		if dnsErr.IsTimeout || dnsErr.IsTemporary {
			transient = true
		}
	})
	return notFound, transient
}

func walkUtlsErrorTree(err error, visit func(error)) {
	if err == nil {
		return
	}
	visit(err)
	if multi, ok := err.(interface{ Unwrap() []error }); ok {
		for _, child := range multi.Unwrap() {
			walkUtlsErrorTree(child, visit)
		}
		return
	}
	if single, ok := err.(interface{ Unwrap() error }); ok {
		walkUtlsErrorTree(single.Unwrap(), visit)
	}
}

func isPermanentUtlsCertificateError(err error) bool {
	permanent := false
	walkUtlsErrorTree(err, func(current error) {
		switch current.(type) {
		case x509.UnknownAuthorityError, *x509.UnknownAuthorityError,
			x509.HostnameError, *x509.HostnameError,
			x509.CertificateInvalidError, *x509.CertificateInvalidError,
			x509.ConstraintViolationError, *x509.ConstraintViolationError,
			x509.InsecureAlgorithmError, *x509.InsecureAlgorithmError,
			x509.SystemRootsError, *x509.SystemRootsError:
			permanent = true
		}
	})
	return permanent
}

func waitForUtlsConnectionRetry(ctx context.Context, delay time.Duration) error {
	timer := time.NewTimer(delay)
	defer func() {
		if !timer.Stop() {
			select {
			case <-timer.C:
			default:
			}
		}
	}()

	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}
