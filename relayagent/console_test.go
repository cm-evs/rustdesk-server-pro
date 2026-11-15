package main

import (
	"testing"
)

func TestParseUSessions(t *testing.T) {
	t.Run("多行正常", func(t *testing.T) {
		out := "1.2.3.4:5000: 12s 3.45MB 100kb/s 50kb/s 80kb/s\n" +
			"5.6.7.8:6000: 60s 10.00MB 200kb/s 90kb/s 120kb/s\n" +
			"9.9.9.9:7000: 5s 0.10MB 50kb/s 20kb/s 45kb/s\n"
		s, total := parseUSessions(out)
		if s != 3 || total != 245 {
			t.Fatalf("sessions=%d total=%d, want 3/245", s, total)
		}
	})
	t.Run("空输出", func(t *testing.T) {
		s, total := parseUSessions("")
		if s != 0 || total != 0 {
			t.Fatalf("sessions=%d total=%d, want 0/0", s, total)
		}
		s, total = parseUSessions("\n\n")
		if s != 0 || total != 0 {
			t.Fatalf("sessions=%d total=%d, want 0/0", s, total)
		}
	})
	t.Run("elapsed为0的行跳过", func(t *testing.T) {
		out := "1.2.3.4:5000: 0s 0.00MB 0kb/s 0kb/s 999kb/s\n" +
			"5.6.7.8:6000: 10s 1.00MB 10kb/s 5kb/s 30kb/s\n"
		s, total := parseUSessions(out)
		if s != 1 || total != 30 {
			t.Fatalf("sessions=%d total=%d, want 1/30", s, total)
		}
	})
	t.Run("格式异常的行跳过", func(t *testing.T) {
		out := "garbage line\n" +
			"1.2.3.4:5000: 12s\n" + // 太短
			"1.2.3.4:5000: xxs 1MB 1kb/s 1kb/s 10kb/s\n" + // elapsed 非法
			"1.2.3.4:5000: 12s 1MB 1kb/s 1kb/s notanumber\n" + // 当前速率非法
			"1.2.3.4:5000: 12s 1MB 1kb/s 1kb/s 7kb/s\n" // 唯一合法行
		s, total := parseUSessions(out)
		if s != 1 || total != 7 {
			t.Fatalf("sessions=%d total=%d, want 1/7", s, total)
		}
	})
	t.Run("小数速率四舍五入", func(t *testing.T) {
		out := "1.2.3.4:5000: 12s 1MB 1kb/s 1kb/s 10.4kb/s\n" +
			"5.6.7.8:6000: 12s 1MB 1kb/s 1kb/s 10.4kb/s\n"
		s, total := parseUSessions(out)
		if s != 2 || total != 21 {
			t.Fatalf("sessions=%d total=%d, want 2/21", s, total)
		}
	})
}

func TestParseLimitValue(t *testing.T) {
	v, err := parseLimitValue("32Mb/s")
	if err != nil || v != 32 {
		t.Fatalf("parse = %v, %v", v, err)
	}
	v, err = parseLimitValue(" 12.5Mb/s\n")
	if err != nil || v != 12.5 {
		t.Fatalf("parse = %v, %v", v, err)
	}
	if _, err := parseLimitValue("abc"); err == nil {
		t.Fatal("invalid value accepted")
	}
}
