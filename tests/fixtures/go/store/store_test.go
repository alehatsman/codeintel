// A test file, so is_test(F) has something to match on by path.
package store

import "testing"

func TestWarm(t *testing.T) {
	s := &Store{entries: map[string]Entry{}}
	if _, ok := Warm(s); ok {
		t.Fatal("empty store returned an entry")
	}
}
