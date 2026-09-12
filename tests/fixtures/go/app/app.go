// Package app is a caller in a third file. Open is exported twice repo-wide,
// so tier A's !ambiguous(N) guard refuses the edge and `calls` has nothing
// here. Tier B resolves it exactly.
package app

import "example.com/fixture/db"

// Start starts up.
func Start() bool {
	return db.Open("sqlite://memory")
}
