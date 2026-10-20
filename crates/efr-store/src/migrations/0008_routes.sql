-- Routing by scope (v0.0.4): a terminal keeps one conversation for each scope key, a
-- registered project or everything outside projects. A row with a tty is the
-- conversation of that terminal's slot for its route. Like every projection, the
-- columns are filled again from the log when the migration rebuilds the projections.
--
-- `route`: the JSON of `efr_protocol::RouteScope`, such as {"kind":"outside"}.
-- `shell_pid`: the pid of the shell that holds the slot, when it said.
-- `routed_seq`: the sequence number of the slot's newest routed event, so the
-- terminal's current slot is its row with the highest one.
ALTER TABLE conversations ADD COLUMN route TEXT CHECK (route IS NULL OR json_valid(route));
ALTER TABLE conversations ADD COLUMN shell_pid INTEGER;
ALTER TABLE conversations ADD COLUMN routed_seq INTEGER;

DROP INDEX conversations_tty;
CREATE INDEX conversations_tty ON conversations (tty, route) WHERE tty IS NOT NULL;
-- The newest conversation of a scope key, for the pick-up of a new terminal.
CREATE INDEX conversations_route ON conversations (route, updated_at) WHERE route IS NOT NULL;
