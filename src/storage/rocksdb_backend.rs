use rocksdb::{DB, Options, WriteBatch, WriteOptions};
use std::path::Path;

pub enum BatchOp {
    Put { key: String, value: Vec<u8> },
    Delete { key: String },
}

pub struct RocksDbBackend {
    db: DB,
    write_opts: WriteOptions,
}

impl RocksDbBackend {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, rocksdb::Error> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        if let Ok(parallelism) = std::thread::available_parallelism() {
            opts.increase_parallelism(parallelism.get() as i32);
        }
        opts.optimize_level_style_compaction(512 * 1024 * 1024);

        let db = DB::open(&opts, path)?;
        let mut write_opts = WriteOptions::default();
        write_opts.set_sync(false); // Default: BATCH mode (async writes)

        Ok(Self { db, write_opts })
    }

    pub fn set_sync(&mut self, sync: bool) {
        self.write_opts.set_sync(sync);
    }

    pub fn put_raw(&self, key: &[u8], value: &[u8]) -> Result<(), rocksdb::Error> {
        self.db.put_opt(key, value, &self.write_opts)
    }

    pub fn get_raw(&self, key: &[u8]) -> Result<Option<Vec<u8>>, rocksdb::Error> {
        self.db.get(key)
    }

    pub fn del_raw(&self, key: &[u8]) -> Result<(), rocksdb::Error> {
        self.db.delete_opt(key, &self.write_opts)
    }

    pub fn apply_batch(&self, ops: &[BatchOp]) -> Result<(), rocksdb::Error> {
        let mut batch = WriteBatch::default();
        for op in ops {
            match op {
                BatchOp::Put { key, value } => {
                    batch.put(key.as_bytes(), value);
                }
                BatchOp::Delete { key } => {
                    batch.delete(key.as_bytes());
                }
            }
        }
        self.db.write_opt(batch, &self.write_opts)
    }

    pub fn iterate_keys<F>(&self, mut callback: F)
    where
        F: FnMut(&[u8]),
    {
        let mut iter = self.db.raw_iterator();
        iter.seek_to_first();
        while iter.valid() {
            if let Some(key) = iter.key() {
                callback(key);
            }
            iter.next();
        }
    }

    pub fn flush(&self) -> Result<(), rocksdb::Error> {
        self.db.flush()
    }
}
