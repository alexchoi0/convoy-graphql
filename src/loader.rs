use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

struct PendingBatch<K, V> {
    keys: Vec<K>,
    senders: Vec<(K, oneshot::Sender<V>)>,
    scheduled: bool,
}

impl<K, V> Default for PendingBatch<K, V> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            senders: Vec::new(),
            scheduled: false,
        }
    }
}

pub struct BatchLoader<K, V, F>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + 'static,
    F: Fn(Vec<K>) -> BoxFuture<'static, HashMap<K, V>> + Send + Sync + Clone + 'static,
{
    delay: Duration,
    loader_fn: F,
    pending: Arc<Mutex<PendingBatch<K, V>>>,
}

impl<K, V, F> BatchLoader<K, V, F>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + 'static,
    F: Fn(Vec<K>) -> BoxFuture<'static, HashMap<K, V>> + Send + Sync + Clone + 'static,
{
    pub fn new(delay: Duration, loader_fn: F) -> Self {
        Self {
            delay,
            loader_fn,
            pending: Arc::new(Mutex::new(PendingBatch::default())),
        }
    }

    pub fn with_delay_ms(delay_ms: u64, loader_fn: F) -> Self {
        Self::new(Duration::from_millis(delay_ms), loader_fn)
    }

    pub async fn load(&self, key: K) -> Option<V> {
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.keys.push(key.clone());
            pending.senders.push((key, tx));

            if !pending.scheduled {
                pending.scheduled = true;

                let pending_clone = self.pending.clone();
                let loader = self.loader_fn.clone();
                let delay = self.delay;

                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;

                    let batch = {
                        let mut p = pending_clone.lock().await;
                        std::mem::take(&mut *p)
                    };

                    if batch.keys.is_empty() {
                        return;
                    }

                    let results = loader(batch.keys).await;

                    for (key, tx) in batch.senders {
                        if let Some(value) = results.get(&key) {
                            let _ = tx.send(value.clone());
                        }
                    }
                });
            }
        }

        rx.await.ok()
    }

    pub async fn load_or_default(&self, key: K) -> V
    where
        V: Default,
    {
        self.load(key).await.unwrap_or_default()
    }
}

impl<K, V, F> Clone for BatchLoader<K, V, F>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + 'static,
    F: Fn(Vec<K>) -> BoxFuture<'static, HashMap<K, V>> + Send + Sync + Clone + 'static,
{
    fn clone(&self) -> Self {
        Self {
            delay: self.delay,
            loader_fn: self.loader_fn.clone(),
            pending: self.pending.clone(),
        }
    }
}

pub struct SimpleBatchLoader<K, V>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + 'static,
{
    delay: Duration,
    pending: Arc<Mutex<PendingBatch<K, V>>>,
}

impl<K, V> SimpleBatchLoader<K, V>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + 'static,
{
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            pending: Arc::new(Mutex::new(PendingBatch::default())),
        }
    }

    pub fn with_delay_ms(delay_ms: u64) -> Self {
        Self::new(Duration::from_millis(delay_ms))
    }

    pub async fn load_with<F, Fut>(&self, key: K, batch_fn: F) -> Option<V>
    where
        F: FnOnce(Vec<K>) -> Fut + Send + 'static,
        Fut: Future<Output = HashMap<K, V>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        let should_spawn = {
            let mut pending = self.pending.lock().await;
            pending.keys.push(key.clone());
            pending.senders.push((key, tx));

            if !pending.scheduled {
                pending.scheduled = true;
                true
            } else {
                false
            }
        };

        if should_spawn {
            let pending_clone = self.pending.clone();
            let delay = self.delay;

            tokio::spawn(async move {
                tokio::time::sleep(delay).await;

                let batch = {
                    let mut p = pending_clone.lock().await;
                    std::mem::take(&mut *p)
                };

                if batch.keys.is_empty() {
                    return;
                }

                let results = batch_fn(batch.keys).await;

                for (key, tx) in batch.senders {
                    if let Some(value) = results.get(&key) {
                        let _ = tx.send(value.clone());
                    }
                }
            });
        }

        rx.await.ok()
    }
}

impl<K, V> Clone for SimpleBatchLoader<K, V>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + 'static,
{
    fn clone(&self) -> Self {
        Self {
            delay: self.delay,
            pending: self.pending.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_batch_loader_batches_requests() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let loader = BatchLoader::with_delay_ms(10, move |keys: Vec<i64>| {
            let cc = call_count_clone.clone();
            Box::pin(async move {
                cc.fetch_add(1, Ordering::SeqCst);
                keys.into_iter()
                    .map(|k| (k, format!("value_{}", k)))
                    .collect()
            })
        });

        let handles: Vec<_> = (0..5)
            .map(|i| {
                let l = loader.clone();
                tokio::spawn(async move { l.load(i).await })
            })
            .collect();

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_some());
        }

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_batch_loader_returns_correct_values() {
        let loader = BatchLoader::with_delay_ms(5, |keys: Vec<i64>| {
            Box::pin(async move { keys.into_iter().map(|k| (k, k * 2)).collect() })
        });

        let result = loader.load(21).await;
        assert_eq!(result, Some(42));
    }

    #[tokio::test]
    async fn test_batch_loader_missing_key() {
        let loader = BatchLoader::with_delay_ms(5, |_keys: Vec<i64>| {
            Box::pin(async move { HashMap::<i64, String>::new() })
        });

        let result = loader.load(1).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_simple_batch_loader() {
        let call_count = Arc::new(AtomicUsize::new(0));

        let loader = SimpleBatchLoader::<i64, String>::with_delay_ms(10);

        let handles: Vec<_> = (0..3)
            .map(|i| {
                let l = loader.clone();
                let cc = call_count.clone();
                tokio::spawn(async move {
                    l.load_with(i, move |keys| {
                        let cc = cc.clone();
                        async move {
                            cc.fetch_add(1, Ordering::SeqCst);
                            keys.into_iter()
                                .map(|k| (k, format!("item_{}", k)))
                                .collect()
                        }
                    })
                    .await
                })
            })
            .collect();

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_some());
        }

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
