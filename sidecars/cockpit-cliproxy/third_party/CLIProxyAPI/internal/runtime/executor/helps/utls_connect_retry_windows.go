//go:build windows

package helps

import (
	"errors"

	"golang.org/x/sys/windows"
)

func isRetryableUtlsSyscallError(err error) bool {
	for _, transient := range []error{
		windows.WSAECONNABORTED,
		windows.WSAECONNREFUSED,
		windows.WSAECONNRESET,
		windows.WSAEHOSTUNREACH,
		windows.WSAENETUNREACH,
		windows.WSAETIMEDOUT,
		windows.ERROR_BROKEN_PIPE,
		windows.ERROR_NETNAME_DELETED,
	} {
		if errors.Is(err, transient) {
			return true
		}
	}
	return false
}
