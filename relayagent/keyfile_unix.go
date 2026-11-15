//go:build !windows

package main

// tightenPrivateKeyFile is a no-op on Unix: writeKeyFile already
// forces 0600 on the private key, which is the whole protection
// there.
func tightenPrivateKeyFile(path string) error {
	return nil
}
