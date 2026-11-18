-- The push token of each enrolled device (v0.0.13, ADR 0028), apart from `devices`, so
-- that table stays as it is. Not a projection: no event holds a token, so a rebuild of
-- the projections leaves this table alone. A revoke deletes the row in its own
-- transaction, and a `DeviceNotRegistered` answer of the Expo push service deletes it
-- when it still holds the token of that push.
CREATE TABLE device_push (
    device_id TEXT PRIMARY KEY REFERENCES devices (device_id),
    -- The Expo push token, `ExponentPushToken[...]`. It is a secret: no log holds it.
    token TEXT NOT NULL CHECK (token LIKE 'ExponentPushToken[%]'),
    updated_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
