//! One learner's authorized view of a class release. The active revision manifest
//! and deletion fence survive a restore of the content cache. No network or storage I/O.
use super::{Hold, Kind, Purpose, Record, RecordRef, Request, Role, MAX_RECORD_BYTES};
use crate::{curriculum::OutcomeId, history::Scope};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Debug, PartialEq, Eq)]
pub enum IndexError {
    Stale,
    WrongScope,
    InvalidRecord,
    InvalidSupport,
    Forgotten,
    Exhausted,
}

struct Head {
    revision: u64,
    /// None after a hold, correction of an ancestor or deletion.
    digest: Option<[u8; 32]>,
    forgotten: bool,
    serial: u64,
    identity: Option<(Option<u64>, OutcomeId, Kind)>,
    supports: BTreeSet<RecordRef>,
}

type Partition = (OutcomeId, Role, Purpose);
type Stamp = (u32, u64, Reverse<RecordRef>);
type EpisodeIndex = BTreeMap<Partition, BTreeSet<Stamp>>;

#[derive(Default)]
pub struct Page {
    pub records: Vec<Record>,
    pub point_lookups: usize,
    pub episode_keys_examined: usize,
}

pub struct MemoryIndex {
    scope: Scope,
    class: u64,
    epoch: u64,
    // Revisions and reverse references are the authoritative restore fence.
    heads: BTreeMap<u64, Head>,
    children: BTreeMap<RecordRef, BTreeSet<u64>>,
    rows: BTreeMap<u64, Record>,
    current: EpisodeIndex,
    carried: EpisodeIndex,
}

impl MemoryIndex {
    pub fn new(scope: Scope, class: u64) -> Self {
        Self {
            scope,
            class,
            epoch: 0,
            heads: BTreeMap::new(),
            children: BTreeMap::new(),
            rows: BTreeMap::new(),
            current: BTreeMap::new(),
            carried: BTreeMap::new(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    fn next_epoch(&self, expected: u64) -> Result<u64, IndexError> {
        if expected != self.epoch {
            return Err(IndexError::Stale);
        }
        self.epoch.checked_add(1).ok_or(IndexError::Exhausted)
    }

    /// Admit a reviewed direct observation/source, or a derived summary citing all
    /// immutable supports. A replacement fences every descendant of the old revision.
    pub fn put(
        &mut self,
        expected: u64,
        record: Record,
        supports: BTreeSet<RecordRef>,
    ) -> Result<(), IndexError> {
        let next = self.next_epoch(expected)?;
        if record.class != self.class
            || record.release != self.scope.revision
            || record.learner.is_some_and(|l| l != self.scope.learner)
        {
            return Err(IndexError::WrongScope);
        }
        let identity = Some((record.learner, record.outcome, record.kind));
        if record.text.len() > MAX_RECORD_BYTES
            || record.grants.is_empty()
            || (record.learner.is_none()
                && !matches!(record.kind, Kind::Material | Kind::Rubric | Kind::Brief))
        {
            return Err(IndexError::InvalidRecord);
        }
        if let Some(prior) = self.heads.get(&record.key.id) {
            if prior.forgotten {
                return Err(IndexError::Forgotten);
            }
            if prior.revision >= record.key.revision || prior.identity != identity {
                return Err(IndexError::InvalidRecord);
            }
        }
        for support in &supports {
            let source = self
                .rows
                .get(&support.id)
                .ok_or(IndexError::InvalidSupport)?;
            if source.key != *support
                || support.id == record.key.id
                || source.outcome != record.outcome
                || (source.term, source.turn) > (record.term, record.turn)
                || (!source.carry && (record.carry || source.term < record.term))
                || (matches!(source.kind, Kind::Rubric | Kind::Response)
                    && record.grants.iter().any(|(_, p)| *p != Purpose::Evaluate))
                || (source.learner.is_some() && source.learner != record.learner)
                || !source.grants.is_superset(&record.grants)
            {
                return Err(IndexError::InvalidSupport);
            }
        }
        // If a proposed replacement depends on its own descendant, removing the
        // old revision would remove an input to the replacement.
        let affected = self.descendants(record.key.id);
        if supports.iter().any(|s| affected.contains(&s.id)) {
            return Err(IndexError::InvalidSupport);
        }
        self.invalidate(affected);
        for parent in &supports {
            self.children
                .entry(*parent)
                .or_default()
                .insert(record.key.id);
        }
        self.heads.insert(
            record.key.id,
            Head {
                revision: record.key.revision,
                digest: Some(record.digest()),
                forgotten: false,
                serial: next,
                identity,
                supports,
            },
        );
        self.install(record);
        self.epoch = next;
        Ok(())
    }

    /// Permanently retire a logical ID in this scope. A late job cannot recreate it.
    pub fn forget(&mut self, expected: u64, key: RecordRef) -> Result<(), IndexError> {
        let next = self.next_epoch(expected)?;
        let head = self.heads.get(&key.id).ok_or(IndexError::InvalidRecord)?;
        if head.revision != key.revision {
            return Err(IndexError::Stale);
        }
        if head.forgotten {
            return Ok(());
        }
        self.invalidate(self.descendants(key.id));
        let tombstone = self.heads.get_mut(&key.id).expect("known record");
        tombstone.forgotten = true;
        tombstone.identity = None;
        self.epoch = next;
        Ok(())
    }

    fn descendants(&self, id: u64) -> BTreeSet<u64> {
        let mut seen = BTreeSet::new();
        let mut work = vec![id];
        while let Some(id) = work.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(h) = self.heads.get(&id) {
                let key = RecordRef {
                    id,
                    revision: h.revision,
                };
                if let Some(children) = self.children.get(&key) {
                    work.extend(children);
                }
            }
        }
        seen
    }

    fn invalidate(&mut self, affected: BTreeSet<u64>) {
        for id in affected {
            let Some(head) = self.heads.get_mut(&id) else {
                continue;
            };
            head.digest = None;
            self.children.remove(&RecordRef {
                id,
                revision: head.revision,
            });
            for parent in std::mem::take(&mut head.supports) {
                if let Some(children) = self.children.get_mut(&parent) {
                    children.remove(&id);
                }
            }
            if let Some(row) = self.rows.remove(&id) {
                self.update_order(&row, false);
            }
        }
    }

    fn install(&mut self, row: Record) {
        self.update_order(&row, true);
        self.rows.insert(row.key.id, row);
    }

    fn update_order(&mut self, row: &Record, insert: bool) {
        if row.kind != Kind::Episode {
            return;
        }
        let stamp = (row.term, row.turn, Reverse(row.key));
        for &(role, purpose) in &row.grants {
            if purpose == Purpose::Author {
                continue;
            }
            let key = (row.outcome, role, purpose);
            for index in
                std::iter::once(&mut self.current).chain(row.carry.then_some(&mut self.carried))
            {
                if insert {
                    index.entry(key).or_default().insert(stamp);
                } else if let Some(bucket) = index.get_mut(&key) {
                    bucket.remove(&stamp);
                }
            }
        }
    }

    /// Replace the body cache from an archive after checking its bytes against the
    /// current manifest. Missing or stale bodies and their descendants stay unavailable.
    /// This offline operation linearizes at a new epoch and may scan the archive.
    pub fn restore(&mut self, expected: u64, archive: &[Record]) -> Result<(), IndexError> {
        let next = self.next_epoch(expected)?;
        let mut verified = BTreeMap::new();
        for row in archive {
            let Some(head) = self.heads.get(&row.key.id) else {
                continue;
            };
            if row.text.len() <= MAX_RECORD_BYTES && head.digest == Some(row.digest()) {
                verified.insert(head.serial, row);
            }
        }
        self.rows.clear();
        self.current.clear();
        self.carried.clear();
        for row in verified.into_values() {
            if self.heads[&row.key.id]
                .supports
                .iter()
                .all(|p| self.rows.get(&p.id).is_some_and(|r| r.key == *p))
            {
                self.install(row.clone());
            }
        }
        self.epoch = next;
        Ok(())
    }

    /// Resolve pins with point reads. For episodes seek directly to the current term,
    /// then fill unused slots with the most recent explicitly carried prior-term rows.
    pub fn page(&self, request: &Request) -> Result<Page, Hold> {
        let s = request.snapshot;
        if s.scope != self.scope || s.class != self.class || s.epoch != self.epoch {
            return Err(Hold::StaleSnapshot);
        }
        request.binding()?;
        let mut page = Page::default();
        let pins: BTreeSet<_> = request
            .required
            .iter()
            .copied()
            .chain(request.memory.into_iter().flatten())
            .collect();
        for key in &pins {
            page.point_lookups += 1;
            if let Some(row) = self
                .rows
                .get(&key.id)
                .filter(|r| r.key == *key && r.visible(s))
            {
                page.records.push(row.clone());
            }
        }
        let bucket = (s.outcome, s.role, s.purpose);
        let min = Reverse(RecordRef {
            id: u64::MAX,
            revision: u64::MAX,
        });
        let max = Reverse(RecordRef { id: 0, revision: 0 });
        let current = (&self.current, (s.term, 0, min), (s.term, s.turn, max));
        let earlier = s
            .term
            .checked_sub(1)
            .map(|t| (&self.carried, (0, 0, min), (t, u64::MAX, max)));
        for (index, lower, upper) in std::iter::once(current).chain(earlier) {
            if let Some(rows) = index.get(&bucket) {
                for &(_, _, Reverse(key)) in rows
                    .range(lower..=upper)
                    .rev()
                    .take(4 - page.episode_keys_examined)
                {
                    page.episode_keys_examined += 1;
                    if !pins.contains(&key) {
                        page.records.push(self.rows[&key.id].clone());
                    }
                }
            }
        }
        Ok(page)
    }
}
