PRAGMA application_id=1381258580;
PRAGMA user_version=4;
CREATE TABLE IF NOT EXISTS visits (
    name TEXT PRIMARY KEY, identity TEXT NOT NULL, mtime TEXT NOT NULL,
    prefix_hash TEXT NOT NULL DEFAULT '', offset INTEGER NOT NULL, ordinal INTEGER NOT NULL, pending TEXT
);
CREATE TABLE IF NOT EXISTS turns (
    source TEXT PRIMARY KEY, visit TEXT NOT NULL REFERENCES visits(name) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL, day TEXT, learner TEXT, tutor TEXT NOT NULL, digest TEXT NOT NULL, recorded INTEGER,
    UNIQUE(visit,ordinal)
);
CREATE INDEX IF NOT EXISTS turns_day ON turns(day);
-- A passage keeps the original speaker boundaries and a normalized 256-float
-- openai/text-embedding-3-small vector, encoded in little-endian order.
CREATE TABLE IF NOT EXISTS passages (
    source TEXT NOT NULL REFERENCES turns(source) ON DELETE CASCADE,
    part INTEGER NOT NULL, learner TEXT, tutor TEXT NOT NULL, body TEXT NOT NULL,
    vector BLOB, PRIMARY KEY(source,part)
);
CREATE INDEX IF NOT EXISTS passages_body ON passages(body);
-- Resolve the earlier source against the current collection when following an edge.
CREATE TABLE IF NOT EXISTS links (
    origin TEXT NOT NULL REFERENCES turns(source) ON DELETE CASCADE,
    target TEXT NOT NULL, digest TEXT NOT NULL, PRIMARY KEY(origin,target)
);
CREATE TABLE IF NOT EXISTS study_turns (
    source TEXT PRIMARY KEY REFERENCES turns(source) ON DELETE CASCADE,
    target TEXT NOT NULL, exercise TEXT NOT NULL, observation TEXT, taught INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS continuations (
    visit TEXT PRIMARY KEY REFERENCES visits(name) ON DELETE CASCADE,
    source TEXT NOT NULL REFERENCES turns(source) ON DELETE CASCADE, state TEXT NOT NULL
);
