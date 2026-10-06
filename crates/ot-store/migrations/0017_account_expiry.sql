-- Temporary and emergency accounts (AC-2(2)): turned off when this passes
-- (72 hours after they are made; fixed). NULL: the account doesn't expire.
ALTER TABLE users ADD COLUMN expires_at_ms INTEGER;
