//! The tables, as the daemon creates them and the readers expect them.
//!
//! Times are integer nanoseconds since the Unix epoch, UTC.  Every
//! table but `source`, `source_report` and `status` is a time series keyed by `at`, the
//! moment the daemon read the row, and holds a row only when what it
//! read had changed: chronyd is polled every second, its sources far
//! less often.

/// Bumped when the tables change shape, or what a column holds.  The
/// daemon refuses a database of a later version rather than write into
/// what it does not understand.
pub const VERSION: i64 = 3;

/// The first schema with chronyd's log files' tables.  A database of
/// schema 1 gains them, empty, on open: no migration but `TABLES`.
pub const LOG_TABLES: i64 = 2;

/// The first schema with `source_report`.  An older database gains it,
/// empty, on open, likewise; its sources' latest reports are their
/// latest samples' until the daemon next polls.
pub const SOURCE_REPORT: i64 = 3;

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

    -- `sources`, as chronyd last reported each source: what the
    -- Sources table shows.  Replaced when anything but the sample's age
    -- changes, and kept when a source is removed.  `source_sample`
    -- keeps the history, of samples only: a source's reach falling to
    -- 0, or its samples dropped by a restart, are here alone.
    CREATE TABLE IF NOT EXISTS source_report (
        source_id                   INTEGER PRIMARY KEY REFERENCES source(id),
        at                          INTEGER NOT NULL,
        poll                        INTEGER NOT NULL,
        stratum                     INTEGER NOT NULL,
        -- As `source_sample`'s.
        state                       TEXT    NOT NULL,
        reachability                INTEGER NOT NULL,
        -- When the last sample arrived, as `source_sample`'s; NULL, as
        -- are the three after it, while chronyd has no sample.
        sample_time                 INTEGER,
        latest_measurement          REAL,
        original_latest_measurement REAL,
        latest_measurement_error    REAL
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

    -- chronyd's own log files, read alongside the control protocol: the
    -- full record of every sample (the protocol gives the last), the
    -- only one of refclock and tempcomp samples, and the history from
    -- before the daemon ran.  Times are to the second but for
    -- refclocks, so the keys include the values: two packets from one
    -- source in a second are two rows, and a line read twice is one.

    -- tracking.log.
    CREATE TABLE IF NOT EXISTS log_tracking (
        at                   INTEGER NOT NULL,
        reference            TEXT    NOT NULL,
        stratum              INTEGER NOT NULL,
        frequency            REAL    NOT NULL,
        skew                 REAL    NOT NULL,
        offset               REAL    NOT NULL,
        -- 'N', '+', '-' or '?'.
        leap                 TEXT    NOT NULL,
        combined_sources     INTEGER NOT NULL,
        offset_sd            REAL    NOT NULL,
        remaining_correction REAL    NOT NULL,
        root_delay           REAL    NOT NULL,
        root_dispersion      REAL    NOT NULL,
        max_error            REAL    NOT NULL,
        UNIQUE (at, reference, offset, frequency)
    ) STRICT;

    -- measurements.log.
    CREATE TABLE IF NOT EXISTS log_measurement (
        at                        INTEGER NOT NULL,
        source                    TEXT    NOT NULL,
        leap                      TEXT    NOT NULL,
        stratum                   INTEGER NOT NULL,
        tests                     INTEGER NOT NULL,
        local_poll                INTEGER NOT NULL,
        remote_poll               INTEGER NOT NULL,
        score                     REAL    NOT NULL,
        offset                    REAL    NOT NULL,
        peer_delay                REAL    NOT NULL,
        peer_dispersion           REAL    NOT NULL,
        root_delay                REAL    NOT NULL,
        root_dispersion           REAL    NOT NULL,
        reference_id              INTEGER NOT NULL,
        mode                      INTEGER NOT NULL,
        interleaved               INTEGER NOT NULL,
        transmit_timestamp_source TEXT    NOT NULL,
        receive_timestamp_source  TEXT    NOT NULL,
        UNIQUE (at, source, offset, peer_delay)
    ) STRICT;
    CREATE INDEX IF NOT EXISTS log_measurement_source ON log_measurement(source, at);

    -- statistics.log.  Frequencies as fractions, as the log has them.
    CREATE TABLE IF NOT EXISTS log_statistics (
        at                 INTEGER NOT NULL,
        source             TEXT    NOT NULL,
        standard_deviation REAL    NOT NULL,
        estimated_offset   REAL    NOT NULL,
        offset_sd          REAL    NOT NULL,
        residual_frequency REAL    NOT NULL,
        skew               REAL    NOT NULL,
        stress             REAL    NOT NULL,
        samples            INTEGER NOT NULL,
        best_start         INTEGER NOT NULL,
        runs               INTEGER NOT NULL,
        asymmetry          REAL    NOT NULL,
        UNIQUE (at, source, samples, estimated_offset)
    ) STRICT;
    CREATE INDEX IF NOT EXISTS log_statistics_source ON log_statistics(source, at);

    -- refclocks.log.  The driver's fields are NULL in a filtered sample.
    CREATE TABLE IF NOT EXISTS refclock_sample (
        at            INTEGER NOT NULL,
        reference     TEXT    NOT NULL,
        driver_polls  INTEGER,
        leap          TEXT    NOT NULL,
        pulse         INTEGER,
        raw_offset    REAL,
        cooked_offset REAL    NOT NULL,
        dispersion    REAL    NOT NULL,
        UNIQUE (at, reference, cooked_offset)
    ) STRICT;

    -- tempcomp.log.
    CREATE TABLE IF NOT EXISTS tempcomp_sample (
        at           INTEGER NOT NULL,
        temperature  REAL    NOT NULL,
        compensation REAL    NOT NULL,
        UNIQUE (at, temperature, compensation)
    ) STRICT;

    -- How far each log file has been read, by inode, which survives
    -- logrotate's renames: a file grown since is read from here, one
    -- of the same length is not read again.
    CREATE TABLE IF NOT EXISTS log_file (
        inode  INTEGER PRIMARY KEY,
        path   TEXT    NOT NULL,
        length INTEGER NOT NULL,
        read   INTEGER NOT NULL
    ) STRICT;
";
