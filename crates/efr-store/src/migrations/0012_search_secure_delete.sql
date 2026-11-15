-- The search index forgets the tokens of a deleted doc at once (decision 5). With
-- `secure-delete`, the FTS5 'delete' of a row removes its tokens from the segments in
-- the same write, and `secure_delete` writes zeros over the pages that it frees.
-- Without it, the segments keep the tokens until a merge. The option stays in
-- `search_fts_config`, so each connection uses it. SQLite before 3.42 cannot read the
-- index after this step. A doc that a delete removed before this step keeps its tokens
-- until a merge.
INSERT INTO search_fts (search_fts, rank) VALUES ('secure-delete', 1);
