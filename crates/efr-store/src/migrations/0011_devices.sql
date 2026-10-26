-- The enrolled devices, such as a phone (the remote milestone). efrd writes a row only
-- after the user confirms the pairing in the terminal. Not a projection: no event holds
-- a device, so a rebuild of the projections leaves this table alone. A revoke keeps
-- the row and sets `revoked_at`; a revoked device never comes back, and a new pairing
-- of the same phone makes a new row.
CREATE TABLE devices (
    -- The `efr_protocol::DeviceId` that efrd minted at the pairing.
    device_id TEXT PRIMARY KEY,
    -- The name that the phone sent, as the terminal showed it at the confirmation.
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    -- The P-256 public key in the uncompressed X9.63 form: 0x04, then X and Y.
    public_key BLOB NOT NULL CHECK (length(public_key) = 65 AND substr(public_key, 1, 1) = x'04'),
    -- The fingerprint of the key that the terminal showed, from efrd.
    key_fingerprint TEXT NOT NULL CHECK (key_fingerprint <> ''),
    -- From Tailscale's whois of the pairing connection: `Node.StableID`,
    -- `UserProfile.ID`, and efrd's own `CurrentTailnet.StableID`. Each later connection
    -- of the device must show the same three.
    node_stable_id TEXT NOT NULL CHECK (node_stable_id <> ''),
    user_id INTEGER NOT NULL,
    tailnet_stable_id TEXT NOT NULL CHECK (tailnet_stable_id <> ''),
    -- The scopes of the device's connections: a JSON array of `ScopeName` wire names.
    scopes TEXT NOT NULL CHECK (json_valid(scopes) AND json_type(scopes) = 'array'),
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER,
    revoked_at INTEGER
) STRICT, WITHOUT ROWID;

-- One device that is not revoked per key, so a key names at most one live device.
CREATE UNIQUE INDEX devices_active_key ON devices (public_key) WHERE revoked_at IS NULL;
