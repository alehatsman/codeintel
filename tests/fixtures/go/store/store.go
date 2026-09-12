// Package store is a struct with methods, so that `parent` has something to
// disagree with the file about.
//
// Every method here is declared at file scope. Nothing lexically encloses
// them; only the receiver says who owns them. That is the case docs/plan.md
// M5 names by hand.
package store

import (
	"fmt"
	str "strings"
)

// Entry is one entry in the store.
type Entry struct {
	Value string
}

// Store is a tiny key-value store.
type Store struct {
	entries map[string]Entry
}

// Get reads one entry.
//
// Two lines of documentation, to prove newlines survive.
func (s *Store) Get(key string) (Entry, bool) {
	found, ok := s.entries[key]
	return found, ok
}

// Put writes one entry. A value receiver where Get took a pointer one, so the
// owner is found through both spellings.
func (s Store) Put(key string, value string) {
	s.entries[key] = Entry{Value: value}
}

func (s *Store) evict() {
	clear(s.entries)
}

// Describe renders an entry. A second type with methods in the same file, so
// that resolving a receiver to the wrong one would show up.
func (e Entry) Describe() string {
	return fmt.Sprintf("%s", str.ToUpper(e.Value))
}

// Warm is a free function that calls a method, so `calls` has an edge to find.
func Warm(s *Store) (Entry, bool) {
	return s.Get("warm")
}
