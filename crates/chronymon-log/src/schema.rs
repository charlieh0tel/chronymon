//! The tables, as the daemon creates them and the readers expect them.
//!
//! Times are integer nanoseconds since the Unix epoch, UTC.  Every
//! table but `source` and `status` is a time series keyed by `at`, the
//! moment the daemon read the row, and holds a row only when what it
//! read had changed: chronyd is polled every second, its sources far
//! less often.

/// Bumped when the tables change shape, or what a column holds.  The
/// daemon refuses a database of a later version rather than write into
/// what it does not understand.
pub const VERSION: i64 = 1;

/// Facts about the database itself, the schema version first.
pub(crate) const META: &str = "CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;";

/// Every other table, as of [`VERSION`].  Each `IF NOT EXISTS`, so a
/// later version's migration adds what an older database lacks.
pub(crate) const TABLES: &str = r"
    -- The daemon's view of chronyd as of its last poll, one row: what
    -- the web view's status strip shows, and how it tells a stale feed
    -- from a quiet one.
    CREATE TABLE IF NOT EXISTS status (
        id          INTEGER PRIMARY KEY CHECK (id = 1),
        polled_at   INTEGER NOT NULL,
        -- 'up', 'down' or 'unsupported'.
        state       TEXT    NOT NULL,
        detail      TEXT,
        chronyd_pid INTEGER
    ) STRICT;

    -- Every source chronyd has reported, by address: an IP address, or
    -- a refclock's reference ID as eight hex digits.
    CREATE TABLE IF NOT EXISTS source (
        id           INTEGER PRIMARY KEY,
        address      TEXT    NOT NULL UNIQUE,
        reference_id INTEGER NOT NULL,
        -- 'client', 'peer' or 'refclock'.
        mode         TEXT    NOT NULL,
        -- As configured (`sourcename`), or a refclock's reference ID
        -- as text.
        name         TEXT,
        first_seen   INTEGER NOT NULL,
        last_seen    INTEGER NOT NULL
    ) STRICT;

    -- `tracking`, one row per clock update.
    CREATE TABLE IF NOT EXISTS tracking (
        at                   INTEGER NOT NULL,
        reference_time       INTEGER NOT NULL UNIQUE,
        reference_id         INTEGER NOT NULL,
        reference_address    TEXT    NOT NULL,
        stratum              INTEGER NOT NULL,
        -- 'normal', 'insert', 'delete' or 'unsynchronized'.
        leap                 TEXT    NOT NULL,
        current_correction   REAL    NOT NULL,
        last_offset          REAL    NOT NULL,
        rms_offset           REAL    NOT NULL,
        frequency            REAL    NOT NULL,
        residual_frequency   REAL    NOT NULL,
        skew                 REAL    NOT NULL,
        root_delay           REAL    NOT NULL,
        root_dispersion      REAL    NOT NULL,
        last_update_interval REAL    NOT NULL
    ) STRICT;
    CREATE INDEX IF NOT EXISTS tracking_at ON tracking(at);

    -- `sources`, one row per new sample from a source.
    CREATE TABLE IF NOT EXISTS source_sample (
        at                          INTEGER NOT NULL,
        source_id                   INTEGER NOT NULL REFERENCES source(id),
        -- When the sample arrived: `at` less its age, to the second.
        sample_time                 INTEGER NOT NULL,
        poll                        INTEGER NOT NULL,
        stratum                     INTEGER NOT NULL,
        -- 'selected', 'nonselectable', 'falseticker', 'jittery',
        -- 'combined' or 'not_combined'.
        state                       TEXT    NOT NULL,
        reachability                INTEGER NOT NULL,
        latest_measurement          REAL    NOT NULL,
        original_latest_measurement REAL    NOT NULL,
        latest_measurement_error    REAL    NOT NULL
    ) STRICT;
    CREATE INDEX IF NOT EXISTS source_sample_source ON source_sample(source_id, at);
    CREATE INDEX IF NOT EXISTS source_sample_at ON source_sample(at);

    -- `sourcestats`, one row per change of chronyd's regression.
    CREATE TABLE IF NOT EXISTS source_stats (
        at                     INTEGER NOT NULL,
        source_id              INTEGER NOT NULL REFERENCES source(id),
        samples                INTEGER NOT NULL,
        runs                   INTEGER NOT NULL,
        span                   INTEGER NOT NULL,
        standard_deviation     REAL    NOT NULL,
        residual_frequency     REAL    NOT NULL,
        skew                   REAL    NOT NULL,
        estimated_offset       REAL    NOT NULL,
        estimated_offset_error REAL    NOT NULL
    ) STRICT;
    CREATE INDEX IF NOT EXISTS source_stats_source ON source_stats(source_id, at);
    CREATE INDEX IF NOT EXISTS source_stats_at ON source_stats(at);

    -- `ntpdata`, one row per valid packet from an NTP source.
    CREATE TABLE IF NOT EXISTS ntp_sample (
        at                INTEGER NOT NULL,
        source_id         INTEGER NOT NULL REFERENCES source(id),
        offset            REAL    NOT NULL,
        peer_delay        REAL    NOT NULL,
        peer_dispersion   REAL    NOT NULL,
        response_time     REAL    NOT NULL,
        jitter_asymmetry  REAL    NOT NULL,
        root_delay        REAL    NOT NULL,
        root_dispersion   REAL    NOT NULL,
        stratum           INTEGER NOT NULL,
        poll              INTEGER NOT NULL,
        precision         INTEGER NOT NULL,
        leap              INTEGER NOT NULL,
        reference_id      INTEGER NOT NULL,
        reference_time    INTEGER NOT NULL,
        tests             INTEGER NOT NULL,
        interleaved       INTEGER NOT NULL,
        authenticated     INTEGER NOT NULL,
        total_transmitted INTEGER NOT NULL,
        total_received    INTEGER NOT NULL,
        total_valid       INTEGER NOT NULL,
        total_good        INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX IF NOT EXISTS ntp_sample_source ON ntp_sample(source_id, at);
    CREATE INDEX IF NOT EXISTS ntp_sample_at ON ntp_sample(at);

    -- `selectdata`, one row per change in how selection treats a source.
    CREATE TABLE IF NOT EXISTS selection (
        at                 INTEGER NOT NULL,
        source_id          INTEGER NOT NULL REFERENCES source(id),
        -- chronyd's state character: '*', '+', 'x', 'N', ...
        state              TEXT    NOT NULL,
        authenticated      INTEGER NOT NULL,
        leap               TEXT    NOT NULL,
        -- RPY_SD_OPTION_* bits.
        configured_options INTEGER NOT NULL,
        effective_options  INTEGER NOT NULL,
        score              REAL    NOT NULL,
        low_limit          REAL    NOT NULL,
        high_limit         REAL    NOT NULL
    ) STRICT;
    CREATE INDEX IF NOT EXISTS selection_source ON selection(source_id, at);
    CREATE INDEX IF NOT EXISTS selection_at ON selection(at);

    -- What happened: restarts, steps, source changes (`EventKind`).
    CREATE TABLE IF NOT EXISTS event (
        at        INTEGER NOT NULL,
        kind      TEXT    NOT NULL,
        source_id INTEGER REFERENCES source(id),
        detail    TEXT
    ) STRICT;
    CREATE INDEX IF NOT EXISTS event_at ON event(at);
";
