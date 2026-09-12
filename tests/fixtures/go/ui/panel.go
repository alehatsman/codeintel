// Package ui is the UI layer. This file deliberately violates the layering
// rule, and docs/plan.md M5 requires that a conformance query over import/3
// finds it and that removing the import returns zero rows with status: ok.
package ui

import (
	"example.com/fixture/db"
	_ "example.com/fixture/net"
)

// Draw draws the panel.
func Draw() bool {
	return db.Open("sqlite://memory")
}
