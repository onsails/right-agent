CREATE TABLE IF NOT EXISTS mcp_http_headers (
    server_name  TEXT NOT NULL,
    header_name  TEXT NOT NULL COLLATE NOCASE,
    header_value TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (server_name, header_name),
    FOREIGN KEY (server_name) REFERENCES mcp_servers(name) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_mcp_http_headers_server
    ON mcp_http_headers(server_name);

CREATE TRIGGER IF NOT EXISTS mcp_servers_delete_http_headers
AFTER DELETE ON mcp_servers
BEGIN
    DELETE FROM mcp_http_headers WHERE server_name = old.name;
END;
