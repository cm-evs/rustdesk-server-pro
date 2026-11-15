//go:build windows

package main

import (
	"encoding/binary"
	"fmt"
	"unsafe"

	"golang.org/x/sys/windows"
)

// tightenPrivateKeyFile restricts the ACL of path to the current user
// + SYSTEM (full control each), with inheritance blocked. Best
// effort: any error is returned and the caller only logs a warning.
//
// x/sys v0.28.0 exports no ACL builder (SetEntriesInAcl is
// unexported), so the ACL is assembled byte by byte: an 8-byte ACL
// header followed by one ACCESS_ALLOWED_ACE per SID, then applied
// with SetNamedSecurityInfo.
func tightenPrivateKeyFile(path string) error {
	tok, err := windows.OpenCurrentProcessToken()
	if err != nil {
		return fmt.Errorf("打开进程令牌失败: %w", err)
	}
	defer tok.Close()
	tu, err := tok.GetTokenUser()
	if err != nil {
		return fmt.Errorf("获取当前用户 SID 失败: %w", err)
	}
	systemSID, err := windows.CreateWellKnownSid(windows.WinLocalSystemSid)
	if err != nil {
		return fmt.Errorf("获取 SYSTEM SID 失败: %w", err)
	}
	acl, keepAlive, err := buildAllowACL(
		[]*windows.SID{tu.User.Sid, systemSID}, windows.GENERIC_ALL)
	if err != nil {
		return err
	}
	_ = keepAlive // hold the ACL's backing buffer until the call returns
	if err := windows.SetNamedSecurityInfo(path, windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION,
		nil, nil, acl, nil); err != nil {
		return fmt.Errorf("设置文件 ACL 失败: %w", err)
	}
	return nil
}

const aclRevision = 2 // ACL_REVISION

// buildAllowACL builds a DACL granting mask to each SID. It returns
// the ACL pointer and its backing buffer; the caller must keep the
// buffer alive while the ACL is in use.
func buildAllowACL(sids []*windows.SID, mask windows.ACCESS_MASK) (*windows.ACL, []byte, error) {
	size := 8 // ACL header
	for _, sid := range sids {
		size += 8 + int(windows.GetLengthSid(sid)) // ACE header + mask + SID
	}
	buf := make([]byte, size)
	buf[0] = aclRevision
	binary.LittleEndian.PutUint16(buf[2:4], uint16(size))
	binary.LittleEndian.PutUint16(buf[4:6], uint16(len(sids)))
	off := 8
	for _, sid := range sids {
		sidLen := int(windows.GetLengthSid(sid))
		aceSize := 8 + sidLen
		buf[off+0] = windows.ACCESS_ALLOWED_ACE_TYPE
		buf[off+1] = 0 // no inheritance flags
		binary.LittleEndian.PutUint16(buf[off+2:off+4], uint16(aceSize))
		binary.LittleEndian.PutUint32(buf[off+4:off+8], uint32(mask))
		sidBytes := unsafe.Slice((*byte)(unsafe.Pointer(sid)), sidLen)
		copy(buf[off+8:off+8+sidLen], sidBytes)
		off += aceSize
	}
	return (*windows.ACL)(unsafe.Pointer(&buf[0])), buf, nil
}
