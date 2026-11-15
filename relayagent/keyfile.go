package main

import (
	"fmt"
	"os"
	"path/filepath"
)

// writeKeyFiles writes the id_ed25519 keypair delivered by
// registration. An existing file whose content differs is first backed
// up to *.bak. The private key is 0600, the public key 0644; on
// Windows the private key ACL is additionally tightened best-effort
// (see tightenPrivateKeyFile).
func writeKeyFiles(dir, privateKey, publicKey string, log *logger) error {
	privPath := filepath.Join(dir, "id_ed25519")
	if err := writeKeyFile(privPath, privateKey, 0o600); err != nil {
		return fmt.Errorf("写入 id_ed25519 失败: %w", err)
	}
	if err := tightenPrivateKeyFile(privPath); err != nil {
		log.Logf("警告：收紧 id_ed25519 权限失败（%v），密钥文件仍可用，请手动检查权限", err)
	}
	if err := writeKeyFile(filepath.Join(dir, "id_ed25519.pub"), publicKey, 0o644); err != nil {
		return fmt.Errorf("写入 id_ed25519.pub 失败: %w", err)
	}
	return nil
}

// writeKeyFile writes content verbatim. An existing file with
// different content is renamed to *.bak first. perm is enforced even
// for an existing file (OpenFile only applies it on creation;
// Chmod is a no-op on Windows).
func writeKeyFile(path, content string, perm os.FileMode) error {
	if old, err := os.ReadFile(path); err == nil && string(old) != content {
		// Windows rename fails over an existing file; drop a stale
		// backup first. On Unix the Remove is harmless.
		_ = os.Remove(path + ".bak")
		if err := os.Rename(path, path+".bak"); err != nil {
			return fmt.Errorf("备份旧密钥失败: %w", err)
		}
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, perm)
	if err != nil {
		return err
	}
	if _, err := f.WriteString(content); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Chmod(path, perm)
}
