//! 解码流水线：固定工作线程池 + 代际取消 + in-flight 去重。
//!
//! 两类任务：
//! - `Full`：整页原图，受 generation 约束，翻页即作废；
//! - `Thumb`：缩略图，只在换书时作废，用来把缩略图条慢慢填满。

use crate::book::{decode_bytes, Book, Reader};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Full,
    Thumb,
}



pub struct Decoded {
    pub idx: usize,
    pub kind: Kind,
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
    pub generation: u64,
    pub book_id: u64,
}

struct Job {
    idx: usize,
    kind: Kind,
    book: Arc<Book>,
    generation: u64,
    book_id: u64,
}

pub struct Decoder {
    job_tx: Sender<Job>,
    done_rx: Receiver<Decoded>,
    full_inflight: HashSet<usize>,
    thumb_inflight: HashSet<usize>,
    generation: Arc<AtomicU64>,
    book_id: Arc<AtomicU64>,
}

impl Decoder {
    pub fn new() -> Self {
        let threads = (std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4))
        .saturating_sub(1)
        .clamp(2, 6);

        let (job_tx, job_rx) = unbounded::<Job>();
        let (done_tx, done_rx) = unbounded::<Decoded>();
        let generation = Arc::new(AtomicU64::new(0));
        let book_id = Arc::new(AtomicU64::new(0));

        for i in 0..threads {
            let job_rx: Receiver<Job> = job_rx.clone();
            let done_tx = done_tx.clone();
            let generation = generation.clone();
            let current_book = book_id.clone();
            std::thread::Builder::new()
                .name(format!("decode-{i}"))
                .spawn(move || {
                    // 每个 worker 各自持有一份打开句柄，只在换书时重建
                    let mut cached: Option<(PathBuf, Reader)> = None;
                    while let Ok(job) = job_rx.recv() {
                        if job.book_id != current_book.load(Ordering::Acquire) {
                            continue; // 换书了
                        }
                        if job.kind == Kind::Full && job.generation < generation.load(Ordering::Acquire)
                        {
                            continue; // 用户已经翻走，丢掉过期整页任务
                        }

                        if cached.as_ref().map(|(p, _)| *p != job.book.path).unwrap_or(true) {
                            match Reader::open(&job.book) {
                                Ok(r) => cached = Some((job.book.path.clone(), r)),
                                Err(_) => continue,
                            }
                        }
                        let Some((_, reader)) = cached.as_mut() else {
                            continue;
                        };
                        let Ok(raw) = reader.read(job.idx) else { continue };
                        let Ok(img) = decode_bytes(&raw.bytes) else { continue };

                        let (size, rgba) = match job.kind {
                            Kind::Full => ([img.width(), img.height()], img.into_raw()),
                            Kind::Thumb => {
                                let (w, h) = (img.width(), img.height());
                                let tw = (w.min(256) as u32).max(1);
                                let th = ((tw as f32 * h as f32 / w as f32).round() as u32).max(1);
                                let small = image::imageops::thumbnail(&img, tw, th);
                                ([small.width(), small.height()], small.into_raw())
                            }
                        };

                        let _ = done_tx.send(Decoded {
                            idx: job.idx,
                            kind: job.kind,
                            size,
                            rgba,
                            generation: job.generation,
                            book_id: job.book_id,
                        });
                    }
                })
                .ok();
        }

        Self {
            job_tx,
            done_rx,
            full_inflight: HashSet::new(),
            thumb_inflight: HashSet::new(),
            generation,
            book_id,
        }
    }

    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// 翻页时调用：让所有在途整页任务失效。
    pub fn bump(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.full_inflight.clear();
    }

    /// 换书时调用：作废所有任务（含缩略图）。
    pub fn new_book(&mut self) -> u64 {
        let id = self.book_id.fetch_add(1, Ordering::AcqRel) + 1;
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.full_inflight.clear();
        self.thumb_inflight.clear();
        id
    }

    pub fn request_full(&mut self, book: &Arc<Book>, idx: usize, book_id: u64) {
        if !self.full_inflight.insert(idx) {
            return;
        }
        if self
            .job_tx
            .send(Job {
                idx,
                kind: Kind::Full,
                book: book.clone(),
                generation: self.current_generation(),
                book_id,
            })
            .is_err()
        {
            self.full_inflight.remove(&idx);
        }
    }

    pub fn request_thumb(&mut self, book: &Arc<Book>, idx: usize, book_id: u64) {
        if !self.thumb_inflight.insert(idx) {
            return;
        }
        if self
            .job_tx
            .send(Job {
                idx,
                kind: Kind::Thumb,
                book: book.clone(),
                generation: self.current_generation(),
                book_id,
            })
            .is_err()
        {
            self.thumb_inflight.remove(&idx);
        }
    }

    /// 取走所有已完成结果（主线程每帧调用一次）。
    pub fn drain(&mut self) -> Vec<Decoded> {
        let mut v = Vec::new();
        while let Ok(d) = self.done_rx.try_recv() {
            match d.kind {
                Kind::Full => {
                    self.full_inflight.remove(&d.idx);
                }
                Kind::Thumb => {
                    self.thumb_inflight.remove(&d.idx);
                }
            }
            v.push(d);
        }
        v
    }
}
