-- Local development only. The bootstrap role remains the migration/seed operator;
-- the server runs as this non-superuser role so RLS is exercised.
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE ROLE markhand_app LOGIN PASSWORD 'markhand_app_dev_only' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
GRANT CONNECT ON DATABASE markhand TO markhand_app;
GRANT USAGE, CREATE ON SCHEMA public TO markhand_app;
