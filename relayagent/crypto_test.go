package main

import (
	"bytes"
	"encoding/hex"
	"strings"
	"testing"
)

// Vectors below were computed independently with Python
// (hashlib.sha256 / hmac) and hardcoded here, so a drift in the
// message layout fails the test instead of silently changing both
// sides.

const (
	testToken = "rrt_" + "abababababababababababababababababababababababababababababababab"
	testTID   = "8c039bbb641727ae"
	testNonce = "22222222222222222222222222222222"
	testProof = "dd217b50700aef74d05d55b4796c430810e804b93f73d46ae908bce9e125c608"
	testHBSig = "aef1f7ca78fe06a4b43dd839a68e01e7e286ee99f72225c26e15471230917ade"
)

func testLinkKey(t *testing.T) []byte {
	t.Helper()
	key, err := hex.DecodeString(strings.Repeat("11", 32))
	if err != nil {
		t.Fatal(err)
	}
	return key
}

func TestTokenID(t *testing.T) {
	if got := tokenID(testToken); got != testTID {
		t.Fatalf("tokenID = %q, want %q", got, testTID)
	}
	// 16 lowercase hex chars.
	if len(testTID) != 16 {
		t.Fatalf("token_id length = %d, want 16", len(testTID))
	}
}

func TestRegisterProofMessage(t *testing.T) {
	want := "relay-register-v1\n" + testTID + "\n" + testNonce + "\nnode1\n1.0.0\n1.2.3.4"
	got := registerProofMessage(testTID, testNonce, "node1", "1.0.0", "1.2.3.4")
	if got != want {
		t.Fatalf("msg = %q, want %q", got, want)
	}
	// Empty name/address are empty segments, not omitted.
	empty := registerProofMessage(testTID, testNonce, "", "1.0.0", "")
	if empty != "relay-register-v1\n"+testTID+"\n"+testNonce+"\n\n1.0.0\n" {
		t.Fatalf("empty-segment msg = %q", empty)
	}
}

func TestRegisterProof(t *testing.T) {
	got := registerProof(testLinkKey(t), testTID, testNonce, "node1", "1.0.0", "1.2.3.4")
	if got != testProof {
		t.Fatalf("proof = %q, want %q", got, testProof)
	}
}

func TestHeartbeatSigMessage(t *testing.T) {
	want := "relay-heartbeat-v1\n12\n1763000000\n" +
		strings.Repeat("33", 16) + "\n" + strings.Repeat("44", 12) + "\n" + strings.Repeat("55", 20)
	got := heartbeatSigMessage(12, 1763000000, strings.Repeat("33", 16), strings.Repeat("44", 12), strings.Repeat("55", 20))
	if got != want {
		t.Fatalf("msg = %q, want %q", got, want)
	}
}

func TestHeartbeatSig(t *testing.T) {
	got := heartbeatSig(testLinkKey(t), 12, 1763000000,
		strings.Repeat("33", 16), strings.Repeat("44", 12), strings.Repeat("55", 20))
	if got != testHBSig {
		t.Fatalf("sig = %q, want %q", got, testHBSig)
	}
}

func TestEncryptDecryptRoundTrip(t *testing.T) {
	key := testLinkKey(t)
	plain := []byte(`{"node_id":12,"config":{"port":21117}}`)
	enc, err := encryptBundle(key, plain)
	if err != nil {
		t.Fatal(err)
	}
	if len(enc.Nonce) != 24 {
		t.Fatalf("nonce hex length = %d, want 24", len(enc.Nonce))
	}
	dec, err := decryptBundle(key, enc)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(dec, plain) {
		t.Fatalf("decrypted = %q, want %q", dec, plain)
	}
	// A fresh nonce per encryption: same plaintext, different bundle.
	enc2, err := encryptBundle(key, plain)
	if err != nil {
		t.Fatal(err)
	}
	if enc2.Nonce == enc.Nonce || enc2.Data == enc.Data {
		t.Fatal("two encryptions produced identical bundles; nonce must be random")
	}
}

func TestDecryptBundleRejects(t *testing.T) {
	key := testLinkKey(t)
	other, err := hex.DecodeString(strings.Repeat("22", 32))
	if err != nil {
		t.Fatal(err)
	}
	enc, err := encryptBundle(key, []byte("hello"))
	if err != nil {
		t.Fatal(err)
	}
	// Wrong key.
	if _, err := decryptBundle(other, enc); err == nil {
		t.Error("decrypt with wrong key unexpectedly succeeded")
	}
	// Tampered ciphertext.
	tampered := enc
	data, _ := hex.DecodeString(tampered.Data)
	data[0] ^= 0xff
	tampered.Data = hex.EncodeToString(data)
	if _, err := decryptBundle(key, tampered); err == nil {
		t.Error("decrypt of tampered data unexpectedly succeeded")
	}
	// Bad nonce length.
	if _, err := decryptBundle(key, bundle{Nonce: "aabb", Data: enc.Data}); err == nil {
		t.Error("decrypt with short nonce unexpectedly succeeded")
	}
	// Non-hex data.
	if _, err := decryptBundle(key, bundle{Nonce: enc.Nonce, Data: "zz"}); err == nil {
		t.Error("decrypt of non-hex data unexpectedly succeeded")
	}
	// Short key.
	if _, err := encryptBundle([]byte("short"), []byte("x")); err == nil {
		t.Error("encrypt with short key unexpectedly succeeded")
	}
}

func TestParseLinkKey(t *testing.T) {
	if _, err := parseLinkKey(strings.Repeat("ab", 32)); err != nil {
		t.Fatalf("valid key rejected: %v", err)
	}
	if _, err := parseLinkKey(strings.Repeat("ab", 31)); err == nil {
		t.Error("31-byte key accepted")
	}
	if _, err := parseLinkKey(strings.Repeat("zz", 32)); err == nil {
		t.Error("non-hex key accepted")
	}
	if _, err := parseLinkKey(""); err == nil {
		t.Error("empty key accepted")
	}
}

func TestRandomHex(t *testing.T) {
	a, err := randomHex(16)
	if err != nil {
		t.Fatal(err)
	}
	b, err := randomHex(16)
	if err != nil {
		t.Fatal(err)
	}
	if len(a) != 32 || len(b) != 32 {
		t.Fatalf("lengths = %d/%d, want 32/32", len(a), len(b))
	}
	if a == b {
		t.Fatal("two random nonces are equal")
	}
}
