ALTER TABLE mcp_servers ADD COLUMN auth_type      TEXT;
ALTER TABLE mcp_servers ADD COLUMN auth_header    TEXT;
ALTER TABLE mcp_servers ADD COLUMN auth_token     TEXT;
ALTER TABLE mcp_servers ADD COLUMN refresh_token  TEXT;
ALTER TABLE mcp_servers ADD COLUMN token_endpoint TEXT;
ALTER TABLE mcp_servers ADD COLUMN client_id      TEXT;
ALTER TABLE mcp_servers ADD COLUMN client_secret  TEXT;
ALTER TABLE mcp_servers ADD COLUMN expires_at     TEXT;
