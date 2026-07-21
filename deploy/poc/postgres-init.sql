-- POC bootstrap: non-superuser application role (password matches deploy/.env.example).
CREATE EXTENSION IF NOT EXISTS pgcrypto;

DO $$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'markhand_app') THEN
    CREATE ROLE markhand_app LOGIN PASSWORD 'markhand_app_poc_change_me'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
  END IF;
END
$$;

GRANT CONNECT ON DATABASE markhand TO markhand_app;
GRANT USAGE, CREATE ON SCHEMA public TO markhand_app;
