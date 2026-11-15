package main

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"strings"
)

// Cryptography for the relay management protocol (standard library
// only): SHA-256, HMAC-SHA256, AES-256-GCM. Hex is always lowercase.
// The link key from the registration link is the long-term shared
// secret; neither the one-time token nor the link key ever appears in
// any request or response.

// tokenID derives the public token identifier: the first 16 hex chars
// (8 bytes) of sha256(token).
func tokenID(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:8])
}

// registerProofMessage builds the exact signed message for
// registration (segments joined by single '\n', no trailing newline).
func registerProofMessage(tid, nonce, name, agentVersion, address string) string {
	return strings.Join([]string{
		"relay-register-v1", tid, nonce, name, agentVersion, address,
	}, "\n")
}

// registerProof computes hex(HMAC-SHA256(linkKey, msg)).
func registerProof(linkKey []byte, tid, nonce, name, agentVersion, address string) string {
	mac := hmac.New(sha256.New, linkKey)
	mac.Write([]byte(registerProofMessage(tid, nonce, name, agentVersion, address)))
	return hex.EncodeToString(mac.Sum(nil))
}

// heartbeatSigMessage builds the signed message for a heartbeat.
func heartbeatSigMessage(nodeID int, ts int64, nonceHex, bundleNonceHex, bundleDataHex string) string {
	return strings.Join([]string{
		"relay-heartbeat-v1",
		fmt.Sprintf("%d", nodeID),
		fmt.Sprintf("%d", ts),
		nonceHex, bundleNonceHex, bundleDataHex,
	}, "\n")
}

// heartbeatSig computes hex(HMAC-SHA256(linkKey, msg)).
func heartbeatSig(linkKey []byte, nodeID int, ts int64, nonceHex, bundleNonceHex, bundleDataHex string) string {
	mac := hmac.New(sha256.New, linkKey)
	mac.Write([]byte(heartbeatSigMessage(nodeID, ts, nonceHex, bundleNonceHex, bundleDataHex)))
	return hex.EncodeToString(mac.Sum(nil))
}

// bundle is an AES-256-GCM ciphertext: a 12-byte nonce (24 hex chars)
// and ciphertext||16-byte tag (hex). It matches the Rust aes-gcm
// crate's output format.
type bundle struct {
	Nonce string `json:"nonce"`
	Data  string `json:"data"`
}

// encryptBundle encrypts plaintext with a fresh random 12-byte nonce.
func encryptBundle(linkKey, plaintext []byte) (bundle, error) {
	block, err := aes.NewCipher(linkKey)
	if err != nil {
		return bundle{}, fmt.Errorf("AES 初始化失败: %w", err)
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return bundle{}, fmt.Errorf("GCM 初始化失败: %w", err)
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return bundle{}, fmt.Errorf("随机数生成失败: %w", err)
	}
	data := gcm.Seal(nil, nonce, plaintext, nil)
	return bundle{
		Nonce: hex.EncodeToString(nonce),
		Data:  hex.EncodeToString(data),
	}, nil
}

// decryptBundle decrypts a bundle. Any failure (bad hex, wrong nonce
// length, authentication failure) is an error.
func decryptBundle(linkKey []byte, b bundle) ([]byte, error) {
	block, err := aes.NewCipher(linkKey)
	if err != nil {
		return nil, fmt.Errorf("AES 初始化失败: %w", err)
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, fmt.Errorf("GCM 初始化失败: %w", err)
	}
	nonce, err := hex.DecodeString(b.Nonce)
	if err != nil {
		return nil, fmt.Errorf("bundle nonce 非法 hex: %w", err)
	}
	if len(nonce) != gcm.NonceSize() {
		return nil, fmt.Errorf("bundle nonce 长度 %d，应为 %d", len(nonce), gcm.NonceSize())
	}
	data, err := hex.DecodeString(b.Data)
	if err != nil {
		return nil, fmt.Errorf("bundle data 非法 hex: %w", err)
	}
	plain, err := gcm.Open(nil, nonce, data, nil)
	if err != nil {
		return nil, fmt.Errorf("bundle 解密失败（可能被篡改或密钥不对）: %w", err)
	}
	return plain, nil
}

// randomHex returns n random bytes as lowercase hex.
func randomHex(n int) (string, error) {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return hex.EncodeToString(b), nil
}

// parseLinkKey decodes a 64-hex link key into 32 bytes.
func parseLinkKey(hexKey string) ([]byte, error) {
	key, err := hex.DecodeString(hexKey)
	if err != nil {
		return nil, fmt.Errorf("key 不是合法 hex: %w", err)
	}
	if len(key) != 32 {
		return nil, fmt.Errorf("key 长度 %d 字节，应为 32 字节", len(key))
	}
	return key, nil
}
