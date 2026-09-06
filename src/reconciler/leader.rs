//! Which replica may reconcile (T-0191, CC-03, CC-55).
//!
//! Several Portal replicas serve the UI, but only one may write to the platform: two
//! reconcilers converging the same repository would fight over every resource. The election
//! is a PostgreSQL advisory lock, which needs no new component and no lease arithmetic. The
//! lock lives on a database session, so a leader that crashes, is evicted or loses its
//! network releases it the moment its connection dies, and the next replica wins the lock on
//! its following tick.
//!
//! The leader holds one connection of the pool for as long as it leads. That is the price of
//! a session-scoped lock; the pool is sized for it.

use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::pool::PoolConnection;
use sqlx::{PgPool, Postgres};
use tokio::sync::Mutex;

/// The advisory lock the reconciler competes for.
///
/// PostgreSQL advisory locks share one 64-bit namespace with everything else in the
/// database, so the key is a literal rather than a hash: `jc_recon` in ASCII, which is
/// readable in `pg_locks` when an operator asks who holds it.
pub const RECONCILER_LOCK_KEY: i64 = 0x6a63_5f72_6563_6f6e;

/// One replica's claim on the reconciler role.
pub struct Leadership {
    pool: PgPool,
    key: i64,
    /// The connection holding the lock. `None` means this replica is a follower.
    held: Mutex<Option<PoolConnection<Postgres>>>,
    /// The same fact, readable without awaiting, for status answers.
    leader: AtomicBool,
}

impl Leadership {
    /// A claim on `key` in the database behind `pool`.
    pub fn new(pool: PgPool, key: i64) -> Self {
        Self {
            pool,
            key,
            held: Mutex::new(None),
            leader: AtomicBool::new(false),
        }
    }

    /// A claim on the reconciler lock.
    pub fn reconciler(pool: PgPool) -> Self {
        Self::new(pool, RECONCILER_LOCK_KEY)
    }

    /// Whether this replica currently holds the lock, as of the last [`Leadership::acquire`].
    pub fn is_leader(&self) -> bool {
        self.leader.load(Ordering::Relaxed)
    }

    /// Takes the lock, or confirms that this replica still holds it.
    ///
    /// Call it before every reconcile rather than once at startup: a lock held by a session
    /// that has since died is not held at all, and only asking the database says so. Losing
    /// the connection demotes this replica instead of failing the call, because a follower is
    /// a correct state and a hard error here would stop the loop that recovers from it.
    pub async fn acquire(&self) -> Result<bool, sqlx::Error> {
        let mut held = self.held.lock().await;

        if let Some(connection) = held.as_mut() {
            match sqlx::query("SELECT 1").execute(&mut **connection).await {
                Ok(_) => return Ok(true),
                Err(err) => {
                    tracing::warn!(error = %err, "the connection holding the reconciler lock died");
                    *held = None;
                    self.leader.store(false, Ordering::Relaxed);
                }
            }
        }

        let mut connection = self.pool.acquire().await?;
        let won: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(self.key)
            .fetch_one(&mut *connection)
            .await?;

        if won {
            *held = Some(connection);
        }
        self.leader.store(won, Ordering::Relaxed);
        Ok(won)
    }

    /// Gives the lock up, so another replica can take it without waiting for this one to die.
    ///
    /// Unlocking before the connection goes back to the pool is not optional: an advisory
    /// lock belongs to the session, and a pooled session handed to the next caller would
    /// still be holding it.
    pub async fn resign(&self) {
        let mut held = self.held.lock().await;
        self.leader.store(false, Ordering::Relaxed);
        if let Some(mut connection) = held.take() {
            if let Err(err) = sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(self.key)
                .execute(&mut *connection)
                .await
            {
                // The connection is dropped either way, and dropping it releases the lock.
                tracing::warn!(error = %err, "could not release the reconciler lock explicitly");
            }
        }
    }
}

impl std::fmt::Debug for Leadership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Leadership")
            .field("key", &self.key)
            .field("leader", &self.is_leader())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_key_is_a_readable_constant() {
        // ASCII, so the key stays positive and readable where `pg_locks` shows it.
        assert_eq!(RECONCILER_LOCK_KEY.to_be_bytes(), *b"jc_recon");
    }
}
