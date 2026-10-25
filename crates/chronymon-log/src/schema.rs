//! The tables, as the daemon creates them and the readers expect them.
//!
//! Times are integer nanoseconds since the Unix epoch, UTC.  Every
//! table but `source`, the reports and `status` is a time series keyed
//! by `at`, the moment the daemon read the row, and holds a row only
//! when what it read had changed: chronyd is polled every second, its
//! sources far less often.

use std::fmt;

use chronymon_control::report::ReferenceId;

/// Bumped when the tables change shape, or what a column holds.  The
/// daemon refuses a database of a later version rather than write into
/// what it does not understand.
pub const VERSION: i64 = 6;

/// The first schema with chronyd's log files' tables.  A database of
/// schema 1 gains them, empty, on open: no migration but `TABLES`.
pub const LOG_TABLES: i64 = 2;

/// The first schema with `source_report`.  An older database gains it,
/// empty, on open, likewise; its sources' latest reports are their
/// latest samples' until the daemon next polls.
pub const SOURCE_REPORT: i64 = 3;

/// The first schema keying a log's lines by the whole line, and its
/// files by device, inode and the bytes before where reading stopped.
/// An older database's log tables are rebuilt on open, their rows
/// kept, and every log file read again from its start: the lines the
/// older keys had taken for one another are found again, and those
/// already in are not written twice.
pub const WHOLE_LINES: i64 = 4;

/// Schema 3's log tables renamed out of [`TABLES`]' way, the indexes
/// that went with them dropped, and the checkpoints, by inode alone,
/// dropped.
pub(crate) const LOG_TABLES_ASIDE: &str = "
    DROP INDEX log_measurement_source;
    DROP INDEX log_statistics_source;
    ALTER TABLE log_tracking RENAME TO old_log_tracking;
    ALTER TABLE log_measurement RENAME TO old_log_measurement;
    ALTER TABLE log_statistics RENAME TO old_log_statistics;
    ALTER TABLE refclock_sample RENAME TO old_refclock_sample;
    ALTER TABLE tempcomp_sample RENAME TO old_tempcomp_sample;
    DROP TABLE log_file;
";

/// Their rows copied into [`TABLES`]' new ones, the old dropped.  The
/// columns are as they were.
pub(crate) const LOG_TABLES_BACK: &str = "
    INSERT INTO log_tracking SELECT * FROM old_log_tracking;
    INSERT INTO log_measurement SELECT * FROM old_log_measurement;
    INSERT INTO log_statistics SELECT * FROM old_log_statistics;
    INSERT INTO refclock_sample SELECT * FROM old_refclock_sample;
    INSERT INTO tempcomp_sample SELECT * FROM old_tempcomp_sample;
    DROP TABLE old_log_tracking;
    DROP TABLE old_log_measurement;
    DROP TABLE old_log_statistics;
    DROP TABLE old_refclock_sample;
    DROP TABLE old_tempcomp_sample;
";

/// The first schema with `tracking_report`, `tracking` without its
/// unique reference time, and indexes on what the reader asks.  An
/// older database's `tracking` is rebuilt on open, its rows kept in
/// order.
pub const TRACKING_REPORT: i64 = 5;

/// An older schema's `tracking` renamed out of [`TABLES`]' way, and its
/// index dropped.
pub(crate) const TRACKING_ASIDE: &str = "
    DROP INDEX IF EXISTS tracking_at;
    ALTER TABLE tracking RENAME TO old_tracking;
";

/// Its rows copied into [`TABLES`]' new one in the order written, the
/// old dropped, and `source_sample`'s indexes by `at`, which nothing
/// reads it by, dropped.  The columns are as they were.
pub(crate) const TRACKING_BACK: &str = "
    INSERT INTO tracking SELECT * FROM old_tracking ORDER BY rowid;
    DROP TABLE old_tracking;
    DROP INDEX IF EXISTS source_sample_source;
    DROP INDEX IF EXISTS source_sample_at;
";

/// The `meta` key of chronyd's refclocks locked to others, as the
/// daemon last read chronyd's configuration: a line per lock, its
/// [`RefclockLock::stored`] text.  None before a daemon read it; empty
/// when none is locked.
pub(crate) const REFCLOCK_LOCKS: &str = "refclock_locks";

/// A refclock locked to another, `refclock ... lock REFID` in chronyd's
/// configuration: its PPS samples are paired with the other's
/// (`chrony.conf` documentation, 4.3 to 4.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefclockLock {
    /// The refclock with the `lock` option.
    pub refclock: ReferenceId,
    /// The refclock it names.
    pub locked_to: ReferenceId,
}

impl RefclockLock {
    /// As `meta` stores it: the two reference IDs in hex, as
    /// `source.address` holds them, a space between.  `50505300
    /// 4E4D4541` is PPS locked to NMEA.
    pub(crate) fn stored(self) -> String {
        format!("{} {}", self.refclock, self.locked_to)
    }

    /// One from its [`stored`](Self::stored) text.
    pub(crate) fn parse(line: &str) -> Option<Self> {
        let (refclock, locked_to) = line.split_once(' ')?;
        let hex = |s: &str| u32::from_str_radix(s, 16).ok().map(ReferenceId);
        Some(Self {
            refclock: hex(refclock)?,
            locked_to: hex(locked_to)?,
        })
    }
}

/// `PPS locked to NMEA`.
impl fmt::Display for RefclockLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |id: ReferenceId| id.text().unwrap_or_else(|| id.to_string());
        write!(
            f,
            "{} locked to {}",
            name(self.refclock),
            name(self.locked_to)
        )
    }
}

/// The first schema keying a log file by its inode alone, not its
/// device too, whose number can change across boots (LVM, btrfs): the
/// bytes before where reading stopped already tell a reused inode.  An
/// older database's `log_file` is rebuilt on open, of each inode the
/// row last written.
pub const INODE_KEY: i64 = 6;

/// An older schema's `log_file` renamed out of [`TABLES`]' way.
pub(crate) const LOG_FILE_ASIDE: &str = "
    ALTER TABLE log_file RENAME TO old_log_file;
";

/// Of each inode, its row last written copied into [`TABLES`]' new
/// `log_file`, the old dropped.  Another device's of the same inode is
/// most likely the same file before a boot renumbered it; if not, its
/// fingerprint differs and it is read again from its start.
pub(crate) const LOG_FILE_BACK: &str = "
    INSERT INTO log_file (inode, path, length, read, fingerprint, rejected)
        SELECT inode, path, length, read, fingerprint, rejected FROM old_log_file
        WHERE rowid IN (SELECT MAX(rowid) FROM old_log_file GROUP BY inode);
    DROP TABLE old_log_file;
";

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

    -- `tracking`, as chronyd last reported it: what the status strip
    -- shows.  Replaced when anything changes.  `tracking` keeps the
    -- history, which cannot say which report is chronyd's latest: its
    -- reference time is 0 while unsynchronized, and goes back with a
    -- backward step.
    CREATE TABLE IF NOT EXISTS tracking_report (
        id                   INTEGER PRIMARY KEY CHECK (id = 1),
        at                   INTEGER NOT NULL,
        -- As `tracking`'s.
        reference_time       INTEGER NOT NULL,
        reference_id         INTEGER NOT NULL,
        reference_address    TEXT    NOT NULL,
        stratum              INTEGER NOT NULL,
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

    -- `tracking`, one row per clock update: its last offset or their
    -- RMS changing, not its reference time alone, which chronyd moves
    -- on every slew (`handle_slew`, chrony 4.8 `reference.c:175`); and
    -- the daemon's first read, unless of the last row's reference time
    -- or update (last offset, RMS and reference), already recorded.  An older database's rows are per
    -- change of the reference time, losing sync included.
    CREATE TABLE IF NOT EXISTS tracking (
        at                   INTEGER NOT NULL,
        -- 0 while chronyd is unsynchronized (`REF_GetReferenceParams`,
        -- chrony 4.8 `reference.c:1216`).
        reference_time       INTEGER NOT NULL,
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
    CREATE INDEX IF NOT EXISTS tracking_reference_time ON tracking(reference_time);

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
    -- By `sample_time`, which the reader places them by, not `at`.
    CREATE INDEX IF NOT EXISTS source_sample_source_time
        ON source_sample(source_id, sample_time);
    CREATE INDEX IF NOT EXISTS source_sample_time ON source_sample(sample_time);

    -- `sourcestats`, one row per change of chronyd's regression: not
    -- of the estimated offset and its error alone, which chronyd
    -- extrapolates to the time of the report (`SST_DoSourcestatsReport`,
    -- chrony 4.8 `sourcestats.c:1019`), so they change every poll.
    -- Those two are as of the poll that found the change.
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
    CREATE INDEX IF NOT EXISTS event_source ON event(source_id, at);

    -- chronyd's own log files, read alongside the control protocol: the
    -- full record of every sample (the protocol gives the last), the
    -- only one of refclock and tempcomp samples, and the history from
    -- before the daemon ran.  Times are to the second but for
    -- refclocks, so each is keyed by the whole line: two packets from
    -- one source in a second are two rows, and a line read twice is
    -- one.

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
        max_error            REAL    NOT NULL
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS log_tracking_line ON log_tracking (at, reference,
        stratum, frequency, skew, offset, leap, combined_sources, offset_sd,
        remaining_correction, root_delay, root_dispersion, max_error);

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
        receive_timestamp_source  TEXT    NOT NULL
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS log_measurement_line ON log_measurement (at, source,
        leap, stratum, tests, local_poll, remote_poll, score, offset, peer_delay,
        peer_dispersion, root_delay, root_dispersion, reference_id, mode, interleaved,
        transmit_timestamp_source, receive_timestamp_source);
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
        asymmetry          REAL    NOT NULL
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS log_statistics_line ON log_statistics (at, source,
        standard_deviation, estimated_offset, offset_sd, residual_frequency, skew, stress,
        samples, best_start, runs, asymmetry);
    CREATE INDEX IF NOT EXISTS log_statistics_source ON log_statistics(source, at);

    -- refclocks.log.  The driver's fields are NULL in a filtered sample,
    -- `-` in the log (`log_sample`, refclock.c, 4.3 to 4.9).
    CREATE TABLE IF NOT EXISTS refclock_sample (
        at            INTEGER NOT NULL,
        reference     TEXT    NOT NULL,
        driver_polls  INTEGER,
        leap          TEXT    NOT NULL,
        pulse         INTEGER,
        raw_offset    REAL,
        cooked_offset REAL    NOT NULL,
        dispersion    REAL    NOT NULL
    ) STRICT;
    -- NULLs are distinct in a key, so each is keyed as the log writes
    -- it, `-`: text, never equal to a number.  A filtered sample read
    -- twice is one row; a raw one with the same values beside it is
    -- another.
    CREATE UNIQUE INDEX IF NOT EXISTS refclock_sample_line ON refclock_sample (at, reference,
        IFNULL(driver_polls, '-'), leap, IFNULL(pulse, '-'), IFNULL(raw_offset, '-'),
        cooked_offset, dispersion);
    CREATE INDEX IF NOT EXISTS refclock_sample_reference ON refclock_sample(reference, at);

    -- tempcomp.log.
    CREATE TABLE IF NOT EXISTS tempcomp_sample (
        at           INTEGER NOT NULL,
        temperature  REAL    NOT NULL,
        compensation REAL    NOT NULL
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS tempcomp_sample_line ON tempcomp_sample (at,
        temperature, compensation);

    -- How far each log file has been read, by inode, which survives
    -- logrotate's renames: a file grown since is read from here, one of
    -- the same length is not read again.  Not by device too, whose
    -- number can change across boots (LVM, btrfs).
    CREATE TABLE IF NOT EXISTS log_file (
        inode       INTEGER PRIMARY KEY,
        path        TEXT    NOT NULL,
        length      INTEGER NOT NULL,
        read        INTEGER NOT NULL,
        -- The bytes just before `read`, a line or two: changed or gone,
        -- the file was truncated and rewritten, or the inode reused, and
        -- it is read from its start.
        fingerprint BLOB    NOT NULL,
        -- 1 when its header was refused: none of this file is read,
        -- however it grows.
        rejected    INTEGER NOT NULL
    ) STRICT;
";
