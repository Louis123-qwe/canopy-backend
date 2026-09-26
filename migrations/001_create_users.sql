-- Canopy: users table for auth (signup/login)
-- Run this against your Neon Postgres database.

CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Speeds up the email lookup on every login/signup check.
-- The UNIQUE constraint above already creates an index, but this
-- makes the intent explicit and protects if the constraint is
-- ever relaxed later.
CREATE INDEX IF NOT EXISTS idx_users_email ON users (email);
