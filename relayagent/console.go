package main

import (
	"fmt"
	"io"
	"math"
	"net"
	"strconv"
	"strings"
	"time"
)

// hbbr loopback console: one TCP connection carries exactly one
// plain-text command line and one reply, with a 1s timeout.
const consoleTimeout = time.Second

func consoleCommand(port int, cmd string) (string, error) {
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), consoleTimeout)
	if err != nil {
		return "", err
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(consoleTimeout))
	if _, err := conn.Write([]byte(cmd + "\n")); err != nil {
		return "", err
	}
	data, err := io.ReadAll(conn)
	if err != nil {
		// hbbr may keep the connection open after replying; a
		// timeout with data already received is a usable reply.
		if ne, ok := err.(net.Error); ok && ne.Timeout() && len(data) > 0 {
			return string(data), nil
		}
		return "", err
	}
	return string(data), nil
}

// parseUSessions parses the reply of the `u` command: one line per
// active relay session, e.g.
// "1.2.3.4:5678: 12s 3.45MB 100kb/s 50kb/s 80kb/s"
// (elapsed, total traffic, peak, average, current). Sessions with
// elapsed 0 are not reported by hbbr and are skipped defensively;
// malformed lines are skipped. Returns the session count and the
// summed current throughput in kb/s.
func parseUSessions(out string) (sessions int, totalKbps int) {
	var total float64
	for _, line := range strings.Split(out, "\n") {
		fields := strings.Fields(strings.TrimSpace(line))
		if len(fields) < 6 {
			continue
		}
		elapsed, err := strconv.ParseFloat(strings.TrimSuffix(fields[1], "s"), 64)
		if err != nil || elapsed <= 0 {
			continue
		}
		cur, err := strconv.ParseFloat(strings.TrimSuffix(fields[len(fields)-1], "kb/s"), 64)
		if err != nil {
			continue
		}
		sessions++
		total += cur
	}
	return sessions, int(math.Round(total))
}

// querySessions asks hbbr for its live session count and total
// throughput. Any failure (not running, console unreachable) is
// returned as an error and the caller reports zeros.
func querySessions(port int) (sessions int, totalKbps int, err error) {
	out, err := consoleCommand(port, "u")
	if err != nil {
		return 0, 0, err
	}
	sessions, totalKbps = parseUSessions(out)
	return sessions, totalKbps, nil
}

// parseLimitValue parses a limit reply such as "32Mb/s".
func parseLimitValue(s string) (float64, error) {
	return strconv.ParseFloat(strings.TrimSpace(strings.TrimSuffix(strings.TrimSpace(s), "Mb/s")), 64)
}

// setLimit pushes one speed limit to the running hbbr (ls/tb/sb).
func setLimit(port int, cmd string, value float64) error {
	_, err := consoleCommand(port, fmt.Sprintf("%s %s", cmd, strconv.FormatFloat(value, 'g', -1, 64)))
	return err
}

// getLimit reads one current speed limit from the running hbbr.
func getLimit(port int, cmd string) (float64, error) {
	out, err := consoleCommand(port, cmd)
	if err != nil {
		return 0, err
	}
	return parseLimitValue(out)
}
