//go:build !windows

package helps

import "syscall"

func cockpitRetryPlatformErrorsForTest() []error {
	return []error{
		syscall.ECONNABORTED,
		syscall.ECONNREFUSED,
		syscall.ECONNRESET,
		syscall.EHOSTUNREACH,
		syscall.ENETUNREACH,
		syscall.EPIPE,
		syscall.ETIMEDOUT,
	}
}

func cockpitRetryPlatformRejectedErrorsForTest() []error {
	return []error{syscall.EHOSTDOWN, syscall.ENETDOWN}
}
