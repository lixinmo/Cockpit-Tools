package helps

import (
	"context"
	"crypto/x509"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"
)

type cockpitRetryTestConnection struct {
	roundTripFn func(*http.Request) (*http.Response, error)
	closeCount  int
	closeErr    error
}

func (c *cockpitRetryTestConnection) RoundTrip(req *http.Request) (*http.Response, error) {
	if c.roundTripFn == nil {
		return &http.Response{StatusCode: http.StatusOK, Body: http.NoBody, Request: req}, nil
	}
	return c.roundTripFn(req)
}

func (c *cockpitRetryTestConnection) Close() error {
	c.closeCount++
	return c.closeErr
}

type cockpitRetryTestBody struct {
	reader     io.Reader
	readCount  int
	closeCount int
	closeErr   error
}

func (b *cockpitRetryTestBody) Read(p []byte) (int, error) {
	b.readCount++
	return b.reader.Read(p)
}

func (b *cockpitRetryTestBody) Close() error {
	b.closeCount++
	return b.closeErr
}

type cockpitRetryTestNetError struct {
	message   string
	timeout   bool
	temporary bool
}

func (e cockpitRetryTestNetError) Error() string   { return e.message }
func (e cockpitRetryTestNetError) Timeout() bool   { return e.timeout }
func (e cockpitRetryTestNetError) Temporary() bool { return e.temporary }

type cockpitRetryFalseTimeoutWrapper struct {
	err error
}

func (e cockpitRetryFalseTimeoutWrapper) Error() string {
	return "false timeout wrapper: " + e.err.Error()
}
func (e cockpitRetryFalseTimeoutWrapper) Timeout() bool { return false }
func (e cockpitRetryFalseTimeoutWrapper) Unwrap() error { return e.err }

type cockpitRetryNonContextDialer struct{}

func (cockpitRetryNonContextDialer) Dial(string, string) (net.Conn, error) {
	return nil, errors.New("unexpected Dial call")
}

type cockpitRetryWriteStartedConn struct {
	*trackedNetConn
	writeStarted chan struct{}
	once         sync.Once
}

func (c *cockpitRetryWriteStartedConn) Write(p []byte) (int, error) {
	c.once.Do(func() { close(c.writeStarted) })
	return c.trackedNetConn.Write(p)
}

func cockpitRetryRecordingWaiter(delays *[]time.Duration) utlsRetryWaiter {
	return func(_ context.Context, delay time.Duration) error {
		*delays = append(*delays, delay)
		return nil
	}
}

func TestCockpitConnectRetry_T01ImmediateSuccess(t *testing.T) {
	wantConnection := &cockpitRetryTestConnection{}
	attempts := 0
	var delays []time.Duration
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			return wantConnection, nil
		},
		waitFn: cockpitRetryRecordingWaiter(&delays),
	}

	connection, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
	if errConnect != nil {
		t.Fatal(errConnect)
	}
	if connection != wantConnection || attempts != 1 {
		t.Fatalf("connection = %p, attempts = %d; want %p, 1", connection, attempts, wantConnection)
	}
	if len(delays) != 0 {
		t.Fatalf("delays = %v, want none", delays)
	}
}

func TestCockpitConnectRetry_T02OneTransientThenSuccess(t *testing.T) {
	wantConnection := &cockpitRetryTestConnection{}
	attempts := 0
	var delays []time.Duration
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			if attempts == 1 {
				return nil, fmt.Errorf("utls: TLS handshake: %w", io.EOF)
			}
			return wantConnection, nil
		},
		waitFn: cockpitRetryRecordingWaiter(&delays),
	}

	connection, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
	if errConnect != nil || connection != wantConnection || attempts != 2 {
		t.Fatalf("connection = %p, error = %v, attempts = %d; want %p, nil, 2", connection, errConnect, attempts, wantConnection)
	}
	if want := []time.Duration{200 * time.Millisecond}; !reflect.DeepEqual(delays, want) {
		t.Fatalf("delays = %v, want %v", delays, want)
	}
}

func TestCockpitConnectRetry_T03TwoTransientThenSuccess(t *testing.T) {
	wantConnection := &cockpitRetryTestConnection{}
	attempts := 0
	var delays []time.Duration
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			if attempts < 3 {
				return nil, fmt.Errorf("utls: TLS handshake: %w", io.ErrUnexpectedEOF)
			}
			return wantConnection, nil
		},
		waitFn: cockpitRetryRecordingWaiter(&delays),
	}

	connection, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
	if errConnect != nil || connection != wantConnection || attempts != 3 {
		t.Fatalf("connection = %p, error = %v, attempts = %d; want %p, nil, 3", connection, errConnect, attempts, wantConnection)
	}
	if want := []time.Duration{200 * time.Millisecond, 400 * time.Millisecond}; !reflect.DeepEqual(delays, want) {
		t.Fatalf("delays = %v, want %v", delays, want)
	}
}

func TestCockpitConnectRetry_T04ExhaustionReturnsExactLastError(t *testing.T) {
	first := fmt.Errorf("proxy read: %w", io.EOF)
	second := fmt.Errorf("TCP closed: %w", net.ErrClosed)
	last := fmt.Errorf("HTTP/2 initialize: %w", io.ErrUnexpectedEOF)
	errorsByAttempt := []error{first, second, last}
	attempts := 0
	var delays []time.Duration
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			errConnect := errorsByAttempt[attempts]
			attempts++
			return nil, errConnect
		},
		waitFn: cockpitRetryRecordingWaiter(&delays),
	}

	connection, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
	if connection != nil || errConnect != last {
		t.Fatalf("connection = %v, error = %v; want nil, exact last error %v", connection, errConnect, last)
	}
	if attempts != 3 {
		t.Fatalf("attempts = %d, want 3", attempts)
	}
	if want := []time.Duration{200 * time.Millisecond, 400 * time.Millisecond}; !reflect.DeepEqual(delays, want) {
		t.Fatalf("delays = %v, want %v", delays, want)
	}
}

func TestCockpitConnectRetry_T05PermanentFirstFailureStops(t *testing.T) {
	permanent := errors.New("proxy CONNECT returned status 407")
	attempts := 0
	var delays []time.Duration
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			return nil, permanent
		},
		waitFn: cockpitRetryRecordingWaiter(&delays),
	}

	_, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
	if errConnect != permanent || attempts != 1 || len(delays) != 0 {
		t.Fatalf("error = %v, attempts = %d, delays = %v; want permanent, 1, none", errConnect, attempts, delays)
	}
}

func TestCockpitConnectRetry_T06PermanentSecondFailureStops(t *testing.T) {
	certificate := x509.HostnameError{Certificate: &x509.Certificate{}, Host: "chatgpt.com"}
	permanent := errors.Join(io.EOF, certificate)
	attempts := 0
	var delays []time.Duration
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			if attempts == 1 {
				return nil, io.EOF
			}
			return nil, permanent
		},
		waitFn: cockpitRetryRecordingWaiter(&delays),
	}

	_, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
	if errConnect != permanent || attempts != 2 {
		t.Fatalf("error = %v, attempts = %d; want exact permanent error, 2", errConnect, attempts)
	}
	if want := []time.Duration{200 * time.Millisecond}; !reflect.DeepEqual(delays, want) {
		t.Fatalf("delays = %v, want %v", delays, want)
	}
}

func TestCockpitConnectRetry_T07UnknownOrNilConnectionStops(t *testing.T) {
	tests := []struct {
		name       string
		connectErr error
		wantErr    error
	}{
		{name: "unknown error", connectErr: errors.New("unclassified connection setup failure")},
		{name: "nil connection and nil error", wantErr: errNilUtlsConnection},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			attempts := 0
			waits := 0
			roundTripper := &utlsRoundTripper{
				connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
					attempts++
					return nil, tt.connectErr
				},
				waitFn: func(context.Context, time.Duration) error {
					waits++
					return nil
				},
			}

			_, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
			wantErr := tt.wantErr
			if wantErr == nil {
				wantErr = tt.connectErr
			}
			if errConnect != wantErr || attempts != 1 || waits != 0 {
				t.Fatalf("error = %v, attempts = %d, waits = %d; want %v, 1, 0", errConnect, attempts, waits, wantErr)
			}
		})
	}
}

func TestCockpitConnectRetry_T08CanceledBeforeFirstAttempt(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	attempts := 0
	waits := 0
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			return nil, io.EOF
		},
		waitFn: func(context.Context, time.Duration) error {
			waits++
			return nil
		},
	}

	_, errConnect := roundTripper.createConnectionWithRetry(ctx, "chatgpt.com", "chatgpt.com:443")
	if !errors.Is(errConnect, context.Canceled) || attempts != 0 || waits != 0 {
		t.Fatalf("error = %v, attempts = %d, waits = %d; want context canceled, 0, 0", errConnect, attempts, waits)
	}
}

func TestCockpitConnectRetry_T09CanceledByConnectionAttempt(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	attempts := 0
	waits := 0
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			cancel()
			return nil, io.EOF
		},
		waitFn: func(context.Context, time.Duration) error {
			waits++
			return nil
		},
	}

	_, errConnect := roundTripper.createConnectionWithRetry(ctx, "chatgpt.com", "chatgpt.com:443")
	if !errors.Is(errConnect, context.Canceled) || attempts != 1 || waits != 0 {
		t.Fatalf("error = %v, attempts = %d, waits = %d; want context canceled, 1, 0", errConnect, attempts, waits)
	}
}

func TestCockpitConnectRetry_T10CanceledDuringBackoff(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	attempts := 0
	waits := 0
	roundTripper := &utlsRoundTripper{
		connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			attempts++
			return nil, io.EOF
		},
		waitFn: func(ctx context.Context, delay time.Duration) error {
			waits++
			cancel()
			return waitForUtlsConnectionRetry(ctx, delay)
		},
	}

	_, errConnect := roundTripper.createConnectionWithRetry(ctx, "chatgpt.com", "chatgpt.com:443")
	if !errors.Is(errConnect, context.Canceled) || attempts != 1 || waits != 1 {
		t.Fatalf("error = %v, attempts = %d, waits = %d; want context canceled, 1, 1", errConnect, attempts, waits)
	}
}

func TestCockpitConnectRetry_T11DeadlineDuringBackoff(t *testing.T) {
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Millisecond)
	defer cancel()
	attempts := 0
	roundTripper := &utlsRoundTripper{connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
		attempts++
		return nil, io.EOF
	}}

	_, errConnect := roundTripper.createConnectionWithRetry(ctx, "chatgpt.com", "chatgpt.com:443")
	if !errors.Is(errConnect, context.DeadlineExceeded) || attempts != 1 {
		t.Fatalf("error = %v, attempts = %d; want deadline exceeded, 1", errConnect, attempts)
	}
}

func TestCockpitConnectRetry_T12TemporaryDNSRetries(t *testing.T) {
	errDNS := &net.DNSError{Err: "temporary resolver failure", Name: "chatgpt.com", IsTemporary: true}
	if !isRetryableUtlsConnectionError(fmt.Errorf("dial: %w", errDNS)) {
		t.Fatal("temporary DNS error was not retryable")
	}
}

func TestCockpitConnectRetry_T13DNSTimeoutRetries(t *testing.T) {
	errDNS := &net.DNSError{Err: "resolver timeout", Name: "chatgpt.com", IsTimeout: true}
	if !isRetryableUtlsConnectionError(fmt.Errorf("dial: %w", errDNS)) {
		t.Fatal("DNS timeout was not retryable")
	}
}

func TestCockpitConnectRetry_T14DNSNotFoundNeverRetries(t *testing.T) {
	errDNS := &net.DNSError{
		Err:         "no such host",
		Name:        "missing.invalid",
		IsTimeout:   true,
		IsTemporary: true,
		IsNotFound:  true,
	}
	if isRetryableUtlsConnectionError(fmt.Errorf("dial: %w", errDNS)) {
		t.Fatal("permanent DNS not-found error was retryable")
	}
	temporary := &net.DNSError{Err: "temporary", Name: "chatgpt.com", IsTemporary: true}
	if isRetryableUtlsConnectionError(errors.Join(temporary, errDNS)) {
		t.Fatal("temporary DNS node hid a joined DNS not-found error")
	}
}

func TestCockpitConnectRetry_T15EOFClassificationUsesTypedCauses(t *testing.T) {
	for _, errRetryable := range []error{
		io.EOF,
		io.ErrUnexpectedEOF,
		fmt.Errorf("utls: TLS handshake: %w", io.EOF),
	} {
		if !isRetryableUtlsConnectionError(errRetryable) {
			t.Errorf("typed EOF error %v was not retryable", errRetryable)
		}
	}
	if isRetryableUtlsConnectionError(errors.New("utls: TLS handshake: EOF")) {
		t.Fatal("string-only EOF error was retryable")
	}
}

func TestCockpitConnectRetry_T16ExplicitConnectionWhitelistRetries(t *testing.T) {
	tests := append(cockpitRetryPlatformErrorsForTest(), io.ErrClosedPipe, net.ErrClosed)
	for _, errTransient := range tests {
		if !isRetryableUtlsConnectionError(&net.OpError{Op: "dial", Net: "tcp", Err: errTransient}) {
			t.Errorf("explicitly allowed connection error %v was not retryable", errTransient)
		}
	}
	for _, errRejected := range cockpitRetryPlatformRejectedErrorsForTest() {
		if isRetryableUtlsConnectionError(&net.OpError{Op: "dial", Net: "tcp", Err: errRejected}) {
			t.Errorf("non-whitelisted platform error %v was retryable", errRejected)
		}
	}
}

func TestCockpitConnectRetry_T17TypedTimeoutRetries(t *testing.T) {
	errTimeout := cockpitRetryTestNetError{message: "temporary timeout", timeout: true}
	if !isRetryableUtlsConnectionError(fmt.Errorf("dial: %w", errTimeout)) {
		t.Fatal("typed timeout was not retryable")
	}
	falseTimeout := cockpitRetryTestNetError{message: "not a timeout"}
	if !isRetryableUtlsConnectionError(cockpitRetryFalseTimeoutWrapper{err: errTimeout}) {
		t.Fatal("false-valued timeout wrapper hid a true timeout cause")
	}
	if !isRetryableUtlsConnectionError(errors.Join(falseTimeout, errTimeout)) {
		t.Fatal("first false-valued timeout node hid a later true timeout node")
	}
}

func TestCockpitConnectRetry_T18WrappersDoNotBroadenClassification(t *testing.T) {
	accepted := []error{
		&net.OpError{Op: "dial", Net: "tcp", Err: io.EOF},
		&url.Error{Op: "Post", URL: "https://chatgpt.com", Err: io.ErrUnexpectedEOF},
		&net.DNSError{Err: "resolver transport reset", Name: "chatgpt.com", UnwrapErr: io.ErrClosedPipe},
		cockpitRetryFalseTimeoutWrapper{err: cockpitRetryTestNetError{message: "inner timeout", timeout: true}},
		fmt.Errorf("outer: %w", net.ErrClosed),
	}
	for _, errAccepted := range accepted {
		if !isRetryableUtlsConnectionError(errAccepted) {
			t.Errorf("wrapper lost allowed inner cause: %T %v", errAccepted, errAccepted)
		}
	}

	rejected := []error{
		cockpitRetryTestNetError{message: "temporary only", temporary: true},
		&net.OpError{Op: "dial", Net: "tcp", Err: errors.New("unclassified")},
		&url.Error{Op: "Post", URL: "https://chatgpt.com", Err: errors.New("EOF while authenticating proxy")},
	}
	for _, errRejected := range rejected {
		if isRetryableUtlsConnectionError(errRejected) {
			t.Errorf("wrapper or Temporary broadened classification: %T %v", errRejected, errRejected)
		}
	}
}

func TestCockpitConnectRetry_T19PermanentTLSAndProxyErrorsStop(t *testing.T) {
	unknownAuthority := x509.UnknownAuthorityError{Cert: &x509.Certificate{}}
	hostname := x509.HostnameError{Certificate: &x509.Certificate{}, Host: "chatgpt.com"}
	invalid := x509.CertificateInvalidError{Cert: &x509.Certificate{}, Reason: x509.Expired}
	constraint := x509.ConstraintViolationError{}
	insecure := x509.InsecureAlgorithmError(x509.MD5WithRSA)
	systemRoots := x509.SystemRootsError{Err: errors.New("system roots unavailable")}
	certificateErrors := []error{
		unknownAuthority, &unknownAuthority,
		hostname, &hostname,
		invalid, &invalid,
		constraint, &constraint,
		insecure, &insecure,
		systemRoots, &systemRoots,
	}
	for _, certificate := range certificateErrors {
		if isRetryableUtlsConnectionError(errors.Join(io.EOF, certificate)) {
			t.Errorf("certificate failure %T did not take priority over joined EOF", certificate)
		}
	}
	if isRetryableUtlsConnectionError(errors.New("proxy CONNECT returned status 407")) {
		t.Fatal("proxy authentication error was retryable")
	}

	waits := 0
	roundTripper := &utlsRoundTripper{
		dialer: cockpitRetryNonContextDialer{},
		waitFn: func(context.Context, time.Duration) error {
			waits++
			return nil
		},
	}
	_, errConnect := roundTripper.createConnectionWithRetry(t.Context(), "chatgpt.com", "chatgpt.com:443")
	if errConnect == nil || !strings.Contains(errConnect.Error(), "does not support context cancellation") || waits != 0 {
		t.Fatalf("error = %v, waits = %d; want permanent dialer config error, 0", errConnect, waits)
	}
}

func TestCockpitConnectRetry_T20SameRequestBodyAndInputsSurvive(t *testing.T) {
	type accountContextKey struct{}
	ctx := context.WithValue(t.Context(), accountContextKey{}, "account-a")
	body := &cockpitRetryTestBody{reader: strings.NewReader("request payload")}
	req, errRequest := http.NewRequestWithContext(ctx, http.MethodPost, "https://chatgpt.com/backend-api/codex/responses", body)
	if errRequest != nil {
		t.Fatal(errRequest)
	}
	req.Header.Set("Authorization", "Bearer account-a-token")
	wantHeader := req.Header.Get("Authorization")

	attempts := 0
	sends := 0
	var delays []time.Duration
	connection := &cockpitRetryTestConnection{roundTripFn: func(gotReq *http.Request) (*http.Response, error) {
		sends++
		if gotReq != req || gotReq.Context() != ctx {
			t.Fatal("request or context changed before HTTP/2 RoundTrip")
		}
		if gotReq.Header.Get("Authorization") != wantHeader {
			t.Fatal("credential header changed during connection retry")
		}
		if body.readCount != 0 || body.closeCount != 0 {
			t.Fatalf("body touched before request send: reads=%d closes=%d", body.readCount, body.closeCount)
		}
		if errClose := gotReq.Body.Close(); errClose != nil {
			t.Fatal(errClose)
		}
		return &http.Response{StatusCode: http.StatusOK, Body: http.NoBody, Request: gotReq}, nil
	}}
	roundTripper := &utlsRoundTripper{
		connectFn: func(gotCtx context.Context, host, addr string) (utlsClientConnection, error) {
			attempts++
			if gotCtx != ctx || host != "chatgpt.com" || addr != "chatgpt.com:443" {
				t.Fatalf("attempt %d inputs = (%v, %q, %q)", attempts, gotCtx, host, addr)
			}
			if body.readCount != 0 || body.closeCount != 0 {
				t.Fatalf("body touched during setup attempt %d", attempts)
			}
			switch attempts {
			case 1:
				return nil, &net.DNSError{Err: "temporary", Name: host, IsTemporary: true}
			case 2:
				return nil, &net.OpError{Op: "dial", Net: "tcp", Err: io.ErrClosedPipe}
			default:
				return connection, nil
			}
		},
		waitFn: cockpitRetryRecordingWaiter(&delays),
	}

	resp, errRoundTrip := roundTripper.RoundTrip(req)
	if errRoundTrip != nil {
		t.Fatal(errRoundTrip)
	}
	if errClose := resp.Body.Close(); errClose != nil {
		t.Fatal(errClose)
	}
	if attempts != 3 || sends != 1 || body.closeCount != 1 || connection.closeCount != 1 {
		t.Fatalf("attempts=%d sends=%d body closes=%d connection closes=%d; want 3,1,1,1", attempts, sends, body.closeCount, connection.closeCount)
	}
	if want := []time.Duration{200 * time.Millisecond, 400 * time.Millisecond}; !reflect.DeepEqual(delays, want) {
		t.Fatalf("delays = %v, want %v", delays, want)
	}
}

func TestCockpitConnectRetry_T21FinalSetupFailureOwnsRequestBody(t *testing.T) {
	t.Run("connect error is preserved when body close fails", func(t *testing.T) {
		closeErr := errors.New("body close failed")
		body := &cockpitRetryTestBody{reader: strings.NewReader("request payload"), closeErr: closeErr}
		req, errRequest := http.NewRequestWithContext(t.Context(), http.MethodPost, "https://chatgpt.com/backend-api/codex/responses", body)
		if errRequest != nil {
			t.Fatal(errRequest)
		}
		last := fmt.Errorf("last setup error: %w", io.ErrUnexpectedEOF)
		attempts := 0
		roundTripper := &utlsRoundTripper{
			connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
				attempts++
				if body.closeCount != 0 {
					t.Fatalf("body closed during setup attempt %d", attempts)
				}
				if attempts == cockpitConnectMaxAttempts {
					return nil, last
				}
				return nil, io.EOF
			},
			waitFn: func(context.Context, time.Duration) error { return nil },
		}

		resp, errRoundTrip := roundTripper.RoundTrip(req)
		if resp != nil || errRoundTrip != last || errors.Is(errRoundTrip, closeErr) {
			t.Fatalf("response = %v, error = %v; want nil and exact final setup error", resp, errRoundTrip)
		}
		if attempts != 3 || body.readCount != 0 || body.closeCount != 1 {
			t.Fatalf("attempts=%d body reads=%d closes=%d; want 3,0,1", attempts, body.readCount, body.closeCount)
		}
	})

	t.Run("pre-send cancellation closes body", func(t *testing.T) {
		body := &cockpitRetryTestBody{reader: strings.NewReader("request payload")}
		ctx, cancel := context.WithCancel(t.Context())
		cancel()
		req, errRequest := http.NewRequestWithContext(ctx, http.MethodPost, "https://chatgpt.com/backend-api/codex/responses", body)
		if errRequest != nil {
			t.Fatal(errRequest)
		}
		roundTripper := &utlsRoundTripper{connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			t.Fatal("connect called after cancellation")
			return nil, nil
		}}

		resp, errRoundTrip := roundTripper.RoundTrip(req)
		if resp != nil || !errors.Is(errRoundTrip, context.Canceled) || body.closeCount != 1 {
			t.Fatalf("response=%v error=%v body closes=%d; want nil, canceled, 1", resp, errRoundTrip, body.closeCount)
		}
	})
}

func TestCockpitConnectRetry_T22PostSendFailureNeverReplaysAndResourcesClose(t *testing.T) {
	t.Run("RoundTrip EOF is not replayed", func(t *testing.T) {
		body := &cockpitRetryTestBody{reader: strings.NewReader("request payload")}
		req, errRequest := http.NewRequestWithContext(t.Context(), http.MethodPost, "https://chatgpt.com/backend-api/codex/responses", body)
		if errRequest != nil {
			t.Fatal(errRequest)
		}
		connects := 0
		sends := 0
		waits := 0
		connection := &cockpitRetryTestConnection{roundTripFn: func(gotReq *http.Request) (*http.Response, error) {
			sends++
			_ = gotReq.Body.Close()
			return nil, io.EOF
		}}
		roundTripper := &utlsRoundTripper{
			connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
				connects++
				return connection, nil
			},
			waitFn: func(context.Context, time.Duration) error {
				waits++
				return nil
			},
		}

		resp, errRoundTrip := roundTripper.RoundTrip(req)
		if resp != nil || !errors.Is(errRoundTrip, io.EOF) {
			t.Fatalf("response = %v, error = %v; want nil, EOF", resp, errRoundTrip)
		}
		if connects != 1 || sends != 1 || waits != 0 || connection.closeCount != 1 || body.closeCount != 1 {
			t.Fatalf("connects=%d sends=%d waits=%d connection closes=%d body closes=%d; want 1,1,0,1,1", connects, sends, waits, connection.closeCount, body.closeCount)
		}
	})

	t.Run("failed partial connection closes before successful retry", func(t *testing.T) {
		requestBody := &cockpitRetryTestBody{reader: strings.NewReader("request payload")}
		responseBody := &cockpitRetryTestBody{reader: strings.NewReader("response payload")}
		req, errRequest := http.NewRequestWithContext(t.Context(), http.MethodPost, "https://chatgpt.com/backend-api/codex/responses", requestBody)
		if errRequest != nil {
			t.Fatal(errRequest)
		}
		failedConnection := &cockpitRetryTestConnection{}
		successConnection := &cockpitRetryTestConnection{roundTripFn: func(gotReq *http.Request) (*http.Response, error) {
			_ = gotReq.Body.Close()
			return &http.Response{StatusCode: http.StatusServiceUnavailable, Body: responseBody, Request: gotReq}, nil
		}}
		connects := 0
		var delays []time.Duration
		roundTripper := &utlsRoundTripper{
			connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
				connects++
				if connects == 1 {
					return failedConnection, io.EOF
				}
				return successConnection, nil
			},
			waitFn: cockpitRetryRecordingWaiter(&delays),
		}

		resp, errRoundTrip := roundTripper.RoundTrip(req)
		if errRoundTrip != nil {
			t.Fatal(errRoundTrip)
		}
		if resp.StatusCode != http.StatusServiceUnavailable || connects != 2 {
			t.Fatalf("status=%d connects=%d; want 503, 2", resp.StatusCode, connects)
		}
		if _, errRead := io.ReadAll(resp.Body); errRead != nil {
			t.Fatal(errRead)
		}
		if errClose := resp.Body.Close(); errClose != nil {
			t.Fatal(errClose)
		}
		if errClose := resp.Body.Close(); errClose != nil {
			t.Fatal(errClose)
		}
		if failedConnection.closeCount != 1 || successConnection.closeCount != 1 || responseBody.closeCount != 1 {
			t.Fatalf("failed connection closes=%d success connection closes=%d response closes=%d; want 1,1,1", failedConnection.closeCount, successConnection.closeCount, responseBody.closeCount)
		}
		if want := []time.Duration{200 * time.Millisecond}; !reflect.DeepEqual(delays, want) {
			t.Fatalf("delays = %v, want %v", delays, want)
		}
	})

	t.Run("production dial closes a partial connection", func(t *testing.T) {
		clientConn, serverConn := net.Pipe()
		defer serverConn.Close()
		trackedConn := &trackedNetConn{Conn: clientConn}
		roundTripper := &utlsRoundTripper{dialer: contextDialerFunc(func(context.Context, string, string) (net.Conn, error) {
			return trackedConn, io.EOF
		})}

		h2Conn, errConnect := roundTripper.createConnection(t.Context(), "chatgpt.com", "chatgpt.com:443")
		if h2Conn != nil || !errors.Is(errConnect, io.EOF) {
			t.Fatalf("connection=%v error=%v; want nil, EOF", h2Conn, errConnect)
		}
		if got := trackedConn.closeCount.Load(); got != 1 {
			t.Fatalf("partial dial connection closes=%d; want 1", got)
		}
	})

	t.Run("HTTP2 initialization honors context cancellation", func(t *testing.T) {
		clientConn, serverConn := net.Pipe()
		defer serverConn.Close()
		trackedConn := &trackedNetConn{Conn: clientConn}
		blockingConn := &cockpitRetryWriteStartedConn{
			trackedNetConn: trackedConn,
			writeStarted:   make(chan struct{}),
		}
		ctx, cancel := context.WithCancel(t.Context())
		connectDone := make(chan error, 1)
		go func() {
			h2Conn, errConnect := newUtlsHTTP2ClientConnection(ctx, blockingConn)
			if h2Conn != nil {
				_ = h2Conn.Close()
			}
			connectDone <- errConnect
		}()

		select {
		case <-blockingConn.writeStarted:
		case <-time.After(time.Second):
			t.Fatal("HTTP/2 initialization did not begin its preface write")
		}
		cancel()
		select {
		case errConnect := <-connectDone:
			if !errors.Is(errConnect, context.Canceled) {
				t.Fatalf("HTTP/2 initialization error=%v; want context canceled", errConnect)
			}
		case <-time.After(time.Second):
			t.Fatal("HTTP/2 initialization did not stop after cancellation")
		}
		if got := trackedConn.closeCount.Load(); got == 0 {
			t.Fatal("blocked HTTP/2 connection was not closed")
		}
	})

	t.Run("empty response closes the connection without replay", func(t *testing.T) {
		req, errRequest := http.NewRequestWithContext(t.Context(), http.MethodGet, "https://chatgpt.com/backend-api/codex/responses", nil)
		if errRequest != nil {
			t.Fatal(errRequest)
		}
		connects := 0
		sends := 0
		connection := &cockpitRetryTestConnection{roundTripFn: func(*http.Request) (*http.Response, error) {
			sends++
			return nil, nil
		}}
		roundTripper := &utlsRoundTripper{connectFn: func(context.Context, string, string) (utlsClientConnection, error) {
			connects++
			return connection, nil
		}}

		resp, errRoundTrip := roundTripper.RoundTrip(req)
		if resp != nil || errRoundTrip == nil || !strings.Contains(errRoundTrip.Error(), "empty response") {
			t.Fatalf("response=%v error=%v; want nil, empty-response error", resp, errRoundTrip)
		}
		if connects != 1 || sends != 1 || connection.closeCount != 1 {
			t.Fatalf("connects=%d sends=%d closes=%d; want 1,1,1", connects, sends, connection.closeCount)
		}
	})
}
