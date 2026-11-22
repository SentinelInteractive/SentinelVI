//! Small bounded caches for decoded resources.
//!
//! The viewer path re-reads the same drawable and texture pages while orbiting
//! and re-shading, so a byte-budgeted cache in front of the decoders removes
//! almost all repeat I/O without unbounded growth.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::rpf9::VirtualResource;
use crate::tpfd::DecodedTexture;

/// Byte-budgeted LRU with two key spaces: resources by file offset, textures by
/// 64-bit asset hash.
pub struct ChunkCache {
    budget: usize,
    used: AtomicUsize,
    tick: Mutex<u64>,
    resources: Mutex<HashMap<u64, (u64, Arc<VirtualResource>)>>,
    textures: Mutex<HashMap<u64, (u64, Arc<DecodedTexture>)>>,
}

impl ChunkCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: AtomicUsize::new(0),
            tick: Mutex::new(0),
            resources: Mutex::new(HashMap::new()),
            textures: Mutex::new(HashMap::new()),
        }
    }

    fn next_tick(&self) -> u64 {
        let mut t = self.tick.lock().unwrap();
        *t += 1;
        *t
    }

    pub fn get_virtual(&self, off: u64) -> Option<Arc<VirtualResource>> {
        let tick = self.next_tick();
        let mut map = self.resources.lock().unwrap();
        let entry = map.get_mut(&off)?;
        entry.0 = tick;
        Some(entry.1.clone())
    }

    pub fn put_virtual(&self, off: u64, res: Arc<VirtualResource>) {
        let tick = self.next_tick();
        let size = res.virt.len() + res.phys.len();
        let mut map = self.resources.lock().unwrap();
        map.insert(off, (tick, res));
        self.used.fetch_add(size, Ordering::Relaxed);
        if self.used.load(Ordering::Relaxed) > self.budget {
            let mut items: Vec<(u64, u64)> = map.iter().map(|(k, v)| (*k, v.0)).collect();
            items.sort_by_key(|(_, t)| *t);
            for (k, _) in items {
                if self.used.load(Ordering::Relaxed) <= self.budget {
                    break;
                }
                if let Some((_, old)) = map.remove(&k) {
                    self.used
                        .fetch_sub(old.virt.len() + old.phys.len(), Ordering::Relaxed);
                }
            }
        }
    }

    pub fn get_texture(&self, hash: u64) -> Option<Arc<DecodedTexture>> {
        let tick = self.next_tick();
        let mut map = self.textures.lock().unwrap();
        let entry = map.get_mut(&hash)?;
        entry.0 = tick;
        Some(entry.1.clone())
    }

    pub fn put_texture(&self, hash: u64, tex: Arc<DecodedTexture>) {
        let tick = self.next_tick();
        let size = tex.rgba.len() + tex.linear.len();
        let mut map = self.textures.lock().unwrap();
        map.insert(hash, (tick, tex));
        self.used.fetch_add(size, Ordering::Relaxed);
        if self.used.load(Ordering::Relaxed) > self.budget {
            let mut items: Vec<(u64, u64)> = map.iter().map(|(k, v)| (*k, v.0)).collect();
            items.sort_by_key(|(_, t)| *t);
            for (k, _) in items {
                if self.used.load(Ordering::Relaxed) <= self.budget {
                    break;
                }
                if let Some((_, old)) = map.remove(&k) {
                    self.used
                        .fetch_sub(old.rgba.len() + old.linear.len(), Ordering::Relaxed);
                }
            }
        }
    }

    pub fn used_bytes(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }
}

impl Default for ChunkCache {
    fn default() -> Self {
        Self::new(512 << 20)
    }
}
