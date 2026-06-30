package zbx_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	zbx "github.com/zebvix/zbx-go"
)

// mockServer returns a test HTTP server that responds with fixed JSON-RPC responses.
func mockServer(t *testing.T, responses map[string]interface{}) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Method string `json:"method"`
			ID     uint64 `json:"id"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, "bad request", 400)
			return
		}
		result, ok := responses[req.Method]
		if !ok {
			w.WriteHeader(200)
			json.NewEncoder(w).Encode(map[string]interface{}{
				"jsonrpc": "2.0", "id": req.ID, "result": nil,
			})
			return
		}
		json.NewEncoder(w).Encode(map[string]interface{}{
			"jsonrpc": "2.0", "id": req.ID, "result": result,
		})
	}))
}

func TestDial(t *testing.T) {
	srv := mockServer(t, map[string]interface{}{
		"eth_chainId": "0x232e", // 8990
	})
	defer srv.Close()

	client, err := zbx.Dial(srv.URL)
	if err != nil {
		t.Fatalf("Dial failed: %v", err)
	}
	defer client.Close()

	if client.ChainID() != zbx.ChainIDTestnet {
		t.Errorf("expected chain ID %d, got %d", zbx.ChainIDTestnet, client.ChainID())
	}
}

func TestGetBlockNumber(t *testing.T) {
	srv := mockServer(t, map[string]interface{}{
		"eth_chainId":     "0x232e",
		"eth_blockNumber": "0x64", // 100
	})
	defer srv.Close()

	client, _ := zbx.Dial(srv.URL)
	num, err := client.GetBlockNumber(context.Background())
	if err != nil {
		t.Fatalf("GetBlockNumber failed: %v", err)
	}
	if num != 100 {
		t.Errorf("expected 100, got %d", num)
	}
}

func TestGetBalance(t *testing.T) {
	srv := mockServer(t, map[string]interface{}{
		"eth_chainId": "0x232e",
		"eth_getBalance": "0xde0b6b3a7640000", // 1 ZBX in wei
	})
	defer srv.Close()

	client, _ := zbx.Dial(srv.URL)
	bal, err := client.GetBalance(context.Background(), "0x1234567890123456789012345678901234567890")
	if err != nil {
		t.Fatalf("GetBalance failed: %v", err)
	}
	if bal == nil || bal.Sign() == 0 {
		t.Error("expected non-zero balance")
	}
}

func TestDialEmptyURL(t *testing.T) {
	_, err := zbx.Dial("")
	if err == nil {
		t.Error("expected error for empty URL")
	}
}

func TestDialNetworkError(t *testing.T) {
	_, err := zbx.Dial("http://127.0.0.1:1") // nothing listening
	if err == nil {
		t.Error("expected error for unreachable endpoint")
	}
}
