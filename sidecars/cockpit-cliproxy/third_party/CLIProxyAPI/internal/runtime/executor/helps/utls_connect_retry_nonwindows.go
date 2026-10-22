//go:build !windows

package helps

import (
	"errors"
	"syscall"
)

func isRetryableUtlsSyscallError(err error) bool {
	for _, transient := range []error{
		syscall.ECONNABORTED,
		syscall.ECONNREFUSED,
		syscall.ECONNRESET,
		syscall.EHOSTUNREACH,
		syscall.ENETUNREACH,
		syscall.EPIPE,
		syscall.ETIMEDOUT,
	} {
		if errors.Is(err, transient) {
			return true
		}
	}
	return false
}
