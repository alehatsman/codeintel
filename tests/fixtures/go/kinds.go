// Package fixture holds one of every kind our Go tags.scm can emit.
//
// This is the fixture behind the kind-fidelity test in docs/plan.md M5:
// upstream's tags.scm emits @definition.type for all four of struct,
// interface, defined type and alias, and `type` is not in our Kind
// vocabulary at all, so every one of them would be an extractor guessing.
package fixture

// Config is a struct, with a documented field.
type Config struct {
	// Limit is how many entries to keep.
	Limit int
	quiet bool
}

// Mode is a defined type over a builtin. Distinct from an alias, which is the
// whole point: `type Mode int` introduces a new type and `type Key = string`
// does not.
type Mode int

// Key is a type alias.
type Key = string

// Handler is an interface, so a method with no body has somewhere to live.
type Handler interface {
	// Handle is the required method.
	Handle(key Key) bool
}

// Limit is a constant.
const Limit = 64

// Banner is a package-level variable.
var Banner = "codeintel"

// Handle satisfies Handler. Its receiver is the only thing that says so: the
// method is lexically at file scope and semantically owned by Config.
func (c Config) Handle(key Key) bool {
	return c.Limit > 0 && len(key) > 0
}

// describe is unexported, so `exported` must not hold for it.
func (c Config) describe() string {
	return Banner
}
