//go:build ignore

// TEST ONLY deterministic Responses API and read-only dumb HTTP Git fixture.
// No real inference, credentials, request-body logging, or external network use.
package main

import (
	"encoding/json"
	"net/http"
	"os"
	"path"
	"strings"
	"time"
)

func main() {
	repo := os.Getenv("FIXTURE_REPO")
	if repo == "" {
		repo = "/var/lib/bridge/reference-fixture/repo"
	}
	files := http.StripPrefix("/repo/", http.FileServer(http.Dir(repo)))
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/repo/") {
			if r.Method != http.MethodGet && r.Method != http.MethodHead {
				http.Error(w, "read only", http.StatusMethodNotAllowed)
				return
			}
			// Only bare-repository files needed by dumb HTTP clone are public.
			p := strings.TrimPrefix(path.Clean(r.URL.Path), "/repo/")
			if p != "HEAD" && p != "info/refs" && !strings.HasPrefix(p, "objects/") {
				http.NotFound(w, r)
				return
			}
			files.ServeHTTP(w, r)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		if r.Method == http.MethodGet && r.URL.Path == "/v1/models" {
			_ = json.NewEncoder(w).Encode(map[string]any{"object": "list", "data": []any{map[string]any{"id": "offline-fixture", "object": "model"}}})
			return
		}
		if r.Method != http.MethodPost || r.URL.Path != "/v1/responses" {
			http.NotFound(w, r)
			return
		}
		var request struct {
			Model string `json:"model"`
		}
		if json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<20)).Decode(&request) != nil {
			http.Error(w, "fixture request refused", http.StatusBadRequest)
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id": "resp_fixture", "object": "response", "created_at": 1,
			"status": "completed", "model": request.Model,
			"output": []any{map[string]any{"type": "message", "id": "msg_fixture",
				"status": "completed", "role": "assistant", "content": []any{
					map[string]any{"type": "output_text", "text": "Offline fixture response.", "annotations": []any{}}}}},
			"usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
		})
	})
	server := &http.Server{Addr: ":8080", Handler: handler, ReadTimeout: 10 * time.Second,
		WriteTimeout: 10 * time.Second, ReadHeaderTimeout: 5 * time.Second, MaxHeaderBytes: 16 << 10}
	if server.ListenAndServe() != http.ErrServerClosed {
		panic("fixture server stopped")
	}
}
