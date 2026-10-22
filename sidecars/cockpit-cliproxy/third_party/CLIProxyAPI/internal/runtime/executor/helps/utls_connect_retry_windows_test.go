//go:build windows

package helps

import "golang.org/x/sys/windows"

func cockpitRetryPlatformErrorsForTest() []error {
	return []error{
		windows.WSAECONNABORTED,
		windows.WSAECONNREFUSED,
		windows.WSAECONNRESET,
		windows.WSAEHOSTUNREACH,
		windows.WSAENETUNREACH,
		windows.WSAETIMEDOUT,
		windows.ERROR_BROKEN_PIPE,
		windows.ERROR_NETNAME_DELETED,
	}
}

func cockpitRetryPlatformRejectedErrorsForTest() []error {
	return []error{windows.WSAEHOSTDOWN, windows.WSAENETDOWN}
}
