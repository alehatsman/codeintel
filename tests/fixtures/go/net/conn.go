// Package net is a second connection layer, exporting a function with the
// same name as db.Open. That ambiguity is the whole point: tier A cannot
// decide which Open a caller in a third file means, and gives up rather than
// guessing. A compiler can.
package net

import "strings"

// Open opens a socket.
func Open(url string) bool {
	return strings.HasPrefix(url, "tcp://")
}
