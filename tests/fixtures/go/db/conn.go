// Package db is the database layer. Nothing here may import from ui/.
package db

// Open opens a connection.
func Open(url string) bool {
	return url != ""
}
