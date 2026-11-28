//! The import run: records from [`Wxr`] into the site tables, in bounded transactions.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io::BufRead;

use cms_db::{Cell, Reply, Sql, Store, TablePrefix};
use cms_outbox::Event;

use crate::error::ImportError;
use crate::ports::{Checkpoint, Fetcher, UploadSink};
use crate::record::{Author, Comment, Item, Meta, Record, Term};
use crate::sql::{Statements, Table, Value};
use crate::wxr::Wxr;

/// Most rows one batch commits: DSQL allows 3,000 modified rows per transaction.
pub const MAX_BATCH_ROWS: u64 = 2_500;
/// Most statement bytes one batch commits: DSQL allows 10 MiB per transaction.
pub const MAX_BATCH_BYTES: u64 = 8 * 1024 * 1024;
/// The reference's uploads directory under a site URL (a data path, matched as text).
const UPLOADS: &str = "/wp-content/uploads/";
/// The reference's zero date, stored for unknown dates.
const ZERO_DATE: &str = "0000-00-00 00:00:00";
/// Meta the reference importer never carries over: an editor's lock on the source site.
const SKIPPED_META: &str = "_edit_lock";

/// How one import run writes.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Options {
    /// The site's table prefix.
    pub prefix: TablePrefix,
    /// The uploaded export's key, named in the outbox event.
    pub source_key: String,
    /// The target site's address. Upload URLs are rewritten to it only when its host differs
    /// from the source site's.
    pub target_url: Option<String>,
    /// `user_registered` for users the import creates.
    pub registered: String,
    /// Row cap per batch; values above [`MAX_BATCH_ROWS`] are lowered to it.
    pub batch_rows: u64,
    /// Byte cap per batch; values above [`MAX_BATCH_BYTES`] are lowered to it.
    pub batch_bytes: u64,
}

impl Options {
    /// Defaults: no URL rewriting, zero registration date, full-size batches.
    pub fn new(prefix: TablePrefix, source_key: impl Into<String>) -> Self {
        Self {
            prefix,
            source_key: source_key.into(),
            target_url: None,
            registered: ZERO_DATE.to_owned(),
            batch_rows: MAX_BATCH_ROWS,
            batch_bytes: MAX_BATCH_BYTES,
        }
    }

    fn caps(&self) -> (u64, u64) {
        (
            self.batch_rows.clamp(1, MAX_BATCH_ROWS),
            self.batch_bytes.clamp(1, MAX_BATCH_BYTES),
        )
    }
}

/// The host side of a run.
pub struct Ports<'a> {
    /// Resume state for this export file.
    pub checkpoint: &'a mut dyn Checkpoint,
    /// Attachment source.
    pub fetcher: &'a mut dyn Fetcher,
    /// Attachment target.
    pub uploads: &'a mut dyn UploadSink,
}

impl fmt::Debug for Ports<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}

/// The outbox event a finished import asks the projector for: `rebuild-all` on the site,
/// payload `{"import": "<source key>"}`. The host writes it with [`cms_outbox::write`].
fn rebuild_event(source_key: &str) -> Event {
    let mut payload = serde_json::Map::new();
    payload.insert("import".to_owned(), source_key.into());
    Event::rebuild_all().with_payload(payload)
}

/// What a run did.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// Records read from the file (site address, authors, terms, items).
    pub records: u64,
    /// Records an earlier run had committed; they were only looked up.
    pub resumed_from: u64,
    /// Users created.
    pub authors_created: u64,
    /// Authors matched to existing users by login.
    pub authors_matched: u64,
    /// Terms created, including terms items link to without declaring them.
    pub terms_created: u64,
    /// Declared terms matched by slug and taxonomy.
    pub terms_matched: u64,
    /// Posts created, by post type.
    pub posts_created: BTreeMap<String, u64>,
    /// Items matched by guid and post type, so not written again.
    pub posts_matched: u64,
    /// Attachments fetched and stored.
    pub attachments_fetched: u64,
    /// Postmeta rows written.
    pub postmeta_rows: u64,
    /// Termmeta rows written.
    pub termmeta_rows: u64,
    /// Comments written.
    pub comments_created: u64,
    /// Commentmeta rows written.
    pub commentmeta_rows: u64,
    /// Term relationship rows written.
    pub links_created: u64,
    /// Parent references resolved after their target appeared later in the file.
    pub fixups: u64,
    /// Transactions committed.
    pub batches: u64,
    /// Highest stored id per table stem (tables with rows only); each identity has been
    /// advanced past it.
    pub max_ids: BTreeMap<&'static str, u64>,
    /// The single outbox event to write once the run returns.
    pub event: Event,
}

/// Imports one export file.
///
/// Records already committed by an earlier run (per the checkpoint) are only looked up, so maps
/// from source ids are rebuilt without writing. Every other record is written unless its natural
/// key (guid plus post type, slug plus taxonomy, login) already exists.
///
/// # Errors
///
/// [`ImportError`] on unreadable input, a store or port failure, or a rejected statement.
/// Batches committed before the failure stay committed and are named by the checkpoint.
pub fn import<S: Store, R: BufRead>(
    store: &mut S,
    ports: Ports<'_>,
    options: &Options,
    input: R,
) -> Result<Report, ImportError> {
    let resumed = ports.checkpoint.load().map_err(ImportError::Checkpoint)?;
    let mut run = Run::new(store, ports, options)?;
    run.report.resumed_from = resumed.unwrap_or(0);
    let mut wxr = Wxr::new(input);
    let mut index = 0_u64;
    while let Some(record) = wxr.next_record()? {
        let replay = index < run.report.resumed_from;
        if !replay {
            run.room(index, record.weight())?;
        }
        match record {
            Record::Site(url) => run.source_url = url,
            Record::Author(author) => run.author(&author, replay, index)?,
            Record::Term(term) => run.term(&term, replay, index)?,
            Record::Item(item) => run.item(&item, replay, index)?,
        }
        index += 1;
    }
    run.finish(index)
}

/// Per-table id allocation: source ids where free, otherwise past the current maximum.
#[derive(Debug, Default)]
struct Ids {
    loaded: u64,
    max: u64,
    claimed: HashSet<u64>,
}

struct Run<'a, 'p, S> {
    store: &'a mut S,
    ports: Ports<'p>,
    options: &'a Options,
    sql: Statements,
    ids: HashMap<Table, Ids>,
    users: HashMap<String, u64>,
    authors: HashMap<u64, u64>,
    terms: HashMap<(String, String), (u64, u64)>,
    /// Terms (taxonomy, slug) declared so far in this file, in file order.
    declared: HashSet<(String, String)>,
    posts: HashMap<u64, u64>,
    post_fixups: Vec<(u64, u64)>,
    term_fixups: Vec<(u64, String, String)>,
    source_url: String,
    open: bool,
    rows: u64,
    bytes: u64,
    report: Report,
}

impl<'a, 'p, S: Store> Run<'a, 'p, S> {
    fn new(store: &'a mut S, ports: Ports<'p>, options: &'a Options) -> Result<Self, ImportError> {
        // The store names its own SQL spelling, so statements cannot disagree with it.
        let sql = Statements::new(store.dialect(), &options.prefix)?;
        Ok(Self {
            store,
            ports,
            options,
            sql,
            ids: HashMap::new(),
            users: HashMap::new(),
            authors: HashMap::new(),
            terms: HashMap::new(),
            declared: HashSet::new(),
            posts: HashMap::new(),
            post_fixups: Vec::new(),
            term_fixups: Vec::new(),
            source_url: String::new(),
            open: false,
            rows: 0,
            bytes: 0,
            report: Report {
                records: 0,
                resumed_from: 0,
                authors_created: 0,
                authors_matched: 0,
                terms_created: 0,
                terms_matched: 0,
                posts_created: BTreeMap::new(),
                posts_matched: 0,
                attachments_fetched: 0,
                postmeta_rows: 0,
                termmeta_rows: 0,
                comments_created: 0,
                commentmeta_rows: 0,
                links_created: 0,
                fixups: 0,
                batches: 0,
                max_ids: BTreeMap::new(),
                event: rebuild_event(&options.source_key),
            },
        })
    }

    // ---- store access ----

    fn query(&mut self, sql: &Sql) -> Result<Reply, ImportError> {
        match self.store.query(sql)? {
            Reply::Failed(error) => Err(ImportError::Server {
                code: error.code(),
                message: Box::from(error.message()),
            }),
            reply => Ok(reply),
        }
    }

    /// Runs one row write inside the open batch and counts it against the batch limits.
    fn write(&mut self, sql: &Sql) -> Result<(), ImportError> {
        self.query(sql)?;
        self.rows += 1;
        self.bytes += u64::try_from(sql.as_bytes().len()).unwrap_or(u64::MAX);
        Ok(())
    }

    /// The first column of every row, as ids. SQL `NULL` (the `MAX` of an empty table) is
    /// skipped.
    fn ids_of(&mut self, sql: &Sql) -> Result<Vec<u64>, ImportError> {
        let reply = self.query(sql)?;
        let unexpected = || ImportError::Reply(String::from_utf8_lossy(sql.as_bytes()).into());
        let Some(set) = reply.as_rows() else {
            return Err(unexpected());
        };
        let mut ids = Vec::with_capacity(set.rows().len());
        for row in set.rows() {
            match row.cells().first() {
                Some(Cell::Bytes(bytes)) => {
                    let id = std::str::from_utf8(bytes)
                        .ok()
                        .and_then(|text| text.parse::<u64>().ok())
                        .ok_or_else(unexpected)?;
                    ids.push(id);
                }
                Some(Cell::Null) => {}
                None => return Err(unexpected()),
            }
        }
        Ok(ids)
    }

    fn first_id(&mut self, sql: &Sql) -> Result<Option<u64>, ImportError> {
        Ok(self.ids_of(sql)?.first().copied())
    }

    // ---- batches ----

    /// Commits the open batch when `weight` would overflow it, then makes sure one is open.
    fn room(&mut self, index: u64, weight: (u64, u64)) -> Result<(), ImportError> {
        if weight == (0, 0) {
            return Ok(());
        }
        let (rows, bytes) = self.options.caps();
        if weight.0 > rows || weight.1 > bytes {
            return Err(ImportError::RecordTooLarge {
                index,
                rows: weight.0,
                bytes: weight.1,
            });
        }
        if self.open && (self.rows + weight.0 > rows || self.bytes + weight.1 > bytes) {
            self.commit(index)?;
        }
        if !self.open {
            self.query(&Statements::begin())?;
            self.open = true;
            self.rows = 0;
            self.bytes = 0;
        }
        Ok(())
    }

    /// Commits the open batch and records the first `done` records as committed.
    fn commit(&mut self, done: u64) -> Result<(), ImportError> {
        self.query(&Statements::commit())?;
        self.open = false;
        self.report.batches += 1;
        self.ports
            .checkpoint
            .save(done)
            .map_err(ImportError::Checkpoint)
    }

    // ---- ids ----

    fn claim(&mut self, table: Table, source: u64) -> Result<u64, ImportError> {
        if !self.ids.contains_key(&table) {
            let sql = self
                .sql
                .max(table)
                .ok_or_else(|| ImportError::Reply(Box::from(table.stem())))?;
            let max = self.first_id(&sql)?.unwrap_or(0);
            self.ids.insert(
                table,
                Ids {
                    loaded: max,
                    max,
                    claimed: HashSet::new(),
                },
            );
        }
        let (loaded, max, taken) = match self.ids.get(&table) {
            Some(ids) => (ids.loaded, ids.max, ids.claimed.contains(&source)),
            None => (0, 0, false),
        };
        // Above the stored maximum and unclaimed by this run means free without a lookup.
        let free = source > 0 && !taken && (source > loaded || !self.exists(table, source)?);
        let id = if free { source } else { max + 1 };
        if let Some(ids) = self.ids.get_mut(&table) {
            ids.max = ids.max.max(id);
            ids.claimed.insert(id);
        }
        Ok(id)
    }

    fn exists(&mut self, table: Table, id: u64) -> Result<bool, ImportError> {
        let column = table.id_column().unwrap_or_default().as_bytes();
        let sql = self
            .sql
            .select(table, column, &[(column, Value::Int(id))], true)?;
        Ok(self.first_id(&sql)?.is_some())
    }

    // ---- authors ----

    fn find_user(&mut self, login: &str) -> Result<Option<u64>, ImportError> {
        if let Some(id) = self.users.get(login) {
            return Ok(Some(*id));
        }
        let sql = self.sql.select(
            Table::Users,
            b"ID",
            &[(b"user_login", Value::Folded(login.as_bytes()))],
            true,
        )?;
        let found = self.first_id(&sql)?;
        if let Some(id) = found {
            self.users.insert(login.to_owned(), id);
        }
        Ok(found)
    }

    fn author(&mut self, author: &Author, replay: bool, index: u64) -> Result<(), ImportError> {
        if let Some(id) = self.find_user(&author.login)? {
            self.authors.insert(author.id, id);
            self.report.authors_matched += 1;
            return Ok(());
        }
        if replay {
            return Err(ImportError::CheckpointMismatch(index));
        }
        let display = if author.display_name.is_empty() {
            &author.login
        } else {
            &author.display_name
        };
        let id = self.create_user(author.id, &author.login, &author.email, display)?;
        self.authors.insert(author.id, id);
        Ok(())
    }

    /// A users row without a password: the person sets one through a reset.
    fn create_user(
        &mut self,
        source: u64,
        login: &str,
        email: &str,
        display: &str,
    ) -> Result<u64, ImportError> {
        use cms_users::{Draft, Field};
        let id = self.claim(Table::Users, source)?;
        let mut draft = Draft::new();
        draft
            .set(Field::Nicename, nicename(login))
            .set(Field::Email, email)
            .set(Field::Registered, &self.options.registered)
            .set(Field::DisplayName, display)
            .set(Field::Login, login);
        let mut cells = vec![(&b"ID"[..], Value::Int(id))];
        cells.extend(
            Field::INSERT
                .iter()
                .map(|field| (field.column(), Value::Text(draft.get(*field)))),
        );
        let sql = self.sql.insert(Table::Users, &cells)?;
        self.write(&sql)?;
        self.users.insert(login.to_owned(), id);
        self.report.authors_created += 1;
        Ok(id)
    }

    /// The target user for an item's creator login, created when no user has it.
    fn creator(&mut self, login: &str) -> Result<u64, ImportError> {
        if login.is_empty() {
            return Ok(0);
        }
        match self.find_user(login)? {
            Some(id) => Ok(id),
            None => self.create_user(0, login, "", login),
        }
    }

    // ---- terms ----

    fn find_term(&mut self, taxonomy: &str, slug: &str) -> Result<Option<(u64, u64)>, ImportError> {
        let key = (taxonomy.to_owned(), slug.to_owned());
        if let Some(found) = self.terms.get(&key) {
            return Ok(Some(*found));
        }
        // A slug can be shared across taxonomies, so each candidate is checked for this one.
        let sql = self.sql.select(
            Table::Terms,
            b"term_id",
            &[(b"slug", Value::Folded(slug.as_bytes()))],
            false,
        )?;
        for term_id in self.ids_of(&sql)? {
            let sql = self.sql.select(
                Table::Taxonomy,
                b"term_taxonomy_id",
                &[
                    (b"term_id", Value::Int(term_id)),
                    (b"taxonomy", Value::Text(taxonomy.as_bytes())),
                ],
                true,
            )?;
            if let Some(taxonomy_id) = self.first_id(&sql)? {
                self.terms.insert(key, (term_id, taxonomy_id));
                return Ok(Some((term_id, taxonomy_id)));
            }
        }
        Ok(None)
    }

    fn term(&mut self, term: &Term, replay: bool, index: u64) -> Result<(), ImportError> {
        let parent_seen = self
            .declared
            .contains(&(term.taxonomy.clone(), term.parent.clone()));
        self.declared
            .insert((term.taxonomy.clone(), term.slug.clone()));
        if let Some((_, taxonomy_id)) = self.find_term(&term.taxonomy, &term.slug)? {
            self.report.terms_matched += 1;
            // A parent declared later in the file is fixed up at the end of the run, which the
            // run that wrote this may not have reached; the parent may already be stored
            // (a later batch committed), so file order decides, not a lookup.
            if replay && !term.parent.is_empty() && !parent_seen {
                self.term_fixups
                    .push((taxonomy_id, term.taxonomy.clone(), term.parent.clone()));
            }
            return Ok(());
        }
        if replay {
            return Err(ImportError::CheckpointMismatch(index));
        }
        self.create_term(term)?;
        Ok(())
    }

    fn create_term(&mut self, term: &Term) -> Result<(u64, u64), ImportError> {
        use cms_taxonomy::{Draft, Field};
        let term_id = self.claim(Table::Terms, term.id)?;
        // Exports carry no taxonomy row id; the reference keeps it equal to the term id.
        let taxonomy_id = self.claim(Table::Taxonomy, term_id)?;
        let parent = if term.parent.is_empty() {
            0
        } else if let Some((parent_id, _)) = self.find_term(&term.taxonomy, &term.parent)? {
            parent_id
        } else {
            self.term_fixups
                .push((taxonomy_id, term.taxonomy.clone(), term.parent.clone()));
            0
        };
        let mut draft = Draft::new();
        draft
            .set(Field::Name, &term.name)
            .set(Field::Slug, &term.slug)
            .set(Field::Taxonomy, &term.taxonomy)
            .set(Field::Description, &term.description);
        let int = |field: Field, value: u64| (field.column(), Value::Int(value));
        let text = |field: Field| (field.column(), Value::Text(draft.get(field)));
        let sql = self.sql.insert(
            Table::Terms,
            &[
                (b"term_id", Value::Int(term_id)),
                text(Field::Name),
                text(Field::Slug),
                int(Field::TermGroup, 0),
            ],
        )?;
        self.write(&sql)?;
        let sql = self.sql.insert(
            Table::Taxonomy,
            &[
                (b"term_taxonomy_id", Value::Int(taxonomy_id)),
                (b"term_id", Value::Int(term_id)),
                text(Field::Taxonomy),
                text(Field::Description),
                int(Field::Parent, parent),
                int(Field::Count, 0),
            ],
        )?;
        self.write(&sql)?;
        self.terms.insert(
            (term.taxonomy.clone(), term.slug.clone()),
            (term_id, taxonomy_id),
        );
        self.report.terms_created += 1;
        self.report.termmeta_rows += self.meta(Table::TermMeta, b"term_id", term_id, &term.meta)?;
        Ok((term_id, taxonomy_id))
    }

    /// Writes meta pairs for `owner`, skipping the source's edit lock. Returns rows written.
    fn meta(
        &mut self,
        table: Table,
        owner_column: &[u8],
        owner: u64,
        pairs: &[Meta],
    ) -> Result<u64, ImportError> {
        let mut written = 0;
        for pair in pairs.iter().filter(|pair| pair.key != SKIPPED_META) {
            let meta_id = self.claim(table, 0)?;
            let sql = self.sql.insert(
                table,
                &[
                    (b"meta_id", Value::Int(meta_id)),
                    (owner_column, Value::Int(owner)),
                    (b"meta_key", Value::Text(pair.key.as_bytes())),
                    (b"meta_value", Value::Text(pair.value.as_bytes())),
                ],
            )?;
            self.write(&sql)?;
            written += 1;
        }
        Ok(written)
    }

    // ---- items ----

    fn find_post(&mut self, guid: &str, kind: &str) -> Result<Option<u64>, ImportError> {
        let sql = self.sql.select(
            Table::Posts,
            b"ID",
            &[
                (b"guid", Value::Text(guid.as_bytes())),
                (b"post_type", Value::Text(kind.as_bytes())),
            ],
            true,
        )?;
        self.first_id(&sql)
    }

    fn item(&mut self, item: &Item, replay: bool, index: u64) -> Result<(), ImportError> {
        if item.guid.is_empty() {
            return Err(ImportError::MissingGuid(item.id));
        }
        let guid = if item.kind == "attachment" {
            self.rewrite(&item.guid)
        } else {
            Cow::Borrowed(item.guid.as_str())
        };
        if let Some(id) = self.find_post(&guid, &item.kind)? {
            self.posts.insert(item.id, id);
            self.report.posts_matched += 1;
            // Same rule as terms: an unresolved parent is fixed up again at the end.
            if replay && item.parent != 0 && !self.posts.contains_key(&item.parent) {
                self.post_fixups.push((id, item.parent));
            }
            return Ok(());
        }
        if replay {
            return Err(ImportError::CheckpointMismatch(index));
        }
        let guid = guid.into_owned();
        self.create_post(item, &guid)
    }

    fn create_post(&mut self, item: &Item, guid: &str) -> Result<(), ImportError> {
        use cms_content::{Draft, Field};
        let author = self.creator(&item.creator)?;
        let id = self.claim(Table::Posts, item.id)?;
        // A parent later in the file is linked once the whole file has been read.
        let parent = match (item.parent, self.posts.get(&item.parent).copied()) {
            (0, _) => 0,
            (_, Some(parent)) => parent,
            (source, None) => {
                self.post_fixups.push((id, source));
                0
            }
        };
        if item.kind == "attachment" && !item.attachment_url.is_empty() {
            self.store_attachment(item)?;
        }
        let content = self.rewrite(&item.content).into_owned();
        let approved = item
            .comments
            .iter()
            .filter(|comment| comment.approved == "1")
            .count();
        let mut draft = Draft::new();
        draft
            .set(Field::Date, &item.date)
            .set(Field::DateGmt, &item.date_gmt)
            .set(Field::Content, &content)
            .set(Field::Title, &item.title)
            .set(Field::Excerpt, &item.excerpt)
            .set(Field::Status, &item.status)
            .set(Field::TypeName, &item.kind)
            .set(Field::CommentStatus, &item.comment_status)
            .set(Field::PingStatus, &item.ping_status)
            .set(Field::Password, &item.password)
            .set(Field::Name, &item.name)
            .set(Field::Modified, &item.modified)
            .set(Field::ModifiedGmt, &item.modified_gmt)
            .set(Field::MimeType, &item.mime_type)
            .set(Field::Guid, guid);
        let mut cells = vec![(&b"ID"[..], Value::Int(id))];
        for field in Field::ALL {
            let value = match field {
                Field::Author => Value::Int(author),
                Field::Parent => Value::Int(parent),
                Field::MenuOrder => Value::Int(item.menu_order),
                other => Value::Text(draft.get(other)),
            };
            cells.push((field.column(), value));
        }
        cells.push((
            b"comment_count",
            Value::Int(u64::try_from(approved).unwrap_or(u64::MAX)),
        ));
        let sql = self.sql.insert(Table::Posts, &cells)?;
        self.write(&sql)?;
        self.posts.insert(item.id, id);
        *self
            .report
            .posts_created
            .entry(item.kind.clone())
            .or_default() += 1;
        self.report.postmeta_rows += self.meta(Table::PostMeta, b"post_id", id, &item.meta)?;
        self.links(item, id)?;
        self.comments(&item.comments, id)
    }

    fn links(&mut self, item: &Item, post: u64) -> Result<(), ImportError> {
        let mut seen = HashSet::new();
        for link in &item.terms {
            if !seen.insert((link.taxonomy.as_str(), link.slug.as_str())) {
                continue;
            }
            // Items may link terms the channel never declared; the reference creates them.
            let (_, taxonomy_id) = match self.find_term(&link.taxonomy, &link.slug)? {
                Some(found) => found,
                None => self.create_term(&Term {
                    taxonomy: link.taxonomy.clone(),
                    slug: link.slug.clone(),
                    name: if link.name.is_empty() {
                        link.slug.clone()
                    } else {
                        link.name.clone()
                    },
                    ..Term::default()
                })?,
            };
            let sql = self.sql.insert(
                Table::Links,
                &[
                    (b"object_id", Value::Int(post)),
                    (b"term_taxonomy_id", Value::Int(taxonomy_id)),
                    (b"term_order", Value::Int(0)),
                ],
            )?;
            self.write(&sql)?;
            self.report.links_created += 1;
        }
        Ok(())
    }

    fn comments(&mut self, comments: &[Comment], post: u64) -> Result<(), ImportError> {
        use cms_comments::{Draft, Field};
        // Ids first, so replies can name parents that come later in the list.
        let mut claimed = Vec::with_capacity(comments.len());
        let mut ids = HashMap::with_capacity(comments.len());
        for comment in comments {
            let id = self.claim(Table::Comments, comment.id)?;
            claimed.push(id);
            ids.entry(comment.id).or_insert(id);
        }
        for (comment, id) in comments.iter().zip(claimed) {
            let parent = ids.get(&comment.parent).copied().unwrap_or(0);
            let user = self.authors.get(&comment.user_id).copied().unwrap_or(0);
            let kind = if comment.kind.is_empty() {
                "comment"
            } else {
                comment.kind.as_str()
            };
            let mut draft = Draft::new();
            draft
                .set(Field::AuthorIp, &comment.author_ip)
                .set(Field::Author, &comment.author)
                .set(Field::AuthorEmail, &comment.author_email)
                .set(Field::AuthorUrl, &comment.author_url)
                .set(Field::Date, &comment.date)
                .set(Field::DateGmt, &comment.date_gmt)
                .set(Field::Content, &comment.content)
                .set(Field::Approved, &comment.approved)
                .set(Field::TypeName, kind);
            let mut cells = vec![(&b"comment_ID"[..], Value::Int(id))];
            for field in Field::INSERT {
                let value = match field {
                    Field::Post => Value::Int(post),
                    Field::Karma => Value::Int(0),
                    Field::Parent => Value::Int(parent),
                    Field::UserId => Value::Int(user),
                    other => Value::Text(draft.get(other)),
                };
                cells.push((field.column(), value));
            }
            let sql = self.sql.insert(Table::Comments, &cells)?;
            self.write(&sql)?;
            self.report.comments_created += 1;
            self.report.commentmeta_rows +=
                self.meta(Table::CommentMeta, b"comment_id", id, &comment.meta)?;
        }
        Ok(())
    }

    // ---- attachments and URLs ----

    fn uploads_base(url: &str) -> String {
        format!("{}{UPLOADS}", url.trim_end_matches('/'))
    }

    /// `text` with the source uploads base replaced by the target's, when the hosts differ.
    fn rewrite<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let Some(target) = self.options.target_url.as_deref() else {
            return Cow::Borrowed(text);
        };
        if self.source_url.is_empty() || host(target) == host(&self.source_url) {
            return Cow::Borrowed(text);
        }
        let from = Self::uploads_base(&self.source_url);
        if !text.contains(&from) {
            return Cow::Borrowed(text);
        }
        Cow::Owned(text.replace(&from, &Self::uploads_base(target)))
    }

    fn store_attachment(&mut self, item: &Item) -> Result<(), ImportError> {
        let base = Self::uploads_base(&self.source_url);
        let relative = item
            .attachment_url
            .strip_prefix(base.as_str())
            .filter(|_| !self.source_url.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                item.meta
                    .iter()
                    .find(|pair| pair.key == "_wp_attached_file" && !pair.value.is_empty())
                    .map(|pair| pair.value.clone())
            })
            .ok_or(ImportError::AttachmentPath(item.id))?;
        let bytes = self
            .ports
            .fetcher
            .fetch(&item.attachment_url)
            .map_err(|source| ImportError::Fetch {
                url: Box::from(item.attachment_url.as_str()),
                source,
            })?;
        self.ports
            .uploads
            .put(&relative, &bytes)
            .map_err(|source| ImportError::Upload {
                path: Box::from(relative.as_str()),
                source,
            })?;
        self.report.attachments_fetched += 1;
        Ok(())
    }

    // ---- end of run ----

    fn fixup_room(&mut self, total: u64) -> Result<(), ImportError> {
        let (rows, _) = self.options.caps();
        if self.open && self.rows + 1 > rows {
            self.commit(total)?;
        }
        if !self.open {
            self.query(&Statements::begin())?;
            self.open = true;
            self.rows = 0;
            self.bytes = 0;
        }
        Ok(())
    }

    fn finish(mut self, total: u64) -> Result<Report, ImportError> {
        for (id, source) in std::mem::take(&mut self.post_fixups) {
            if let Some(parent) = self.posts.get(&source).copied() {
                self.fixup_room(total)?;
                let sql = self.sql.update(
                    Table::Posts,
                    &[(b"post_parent", Value::Int(parent))],
                    b"ID",
                    id,
                )?;
                self.write(&sql)?;
                self.report.fixups += 1;
            }
        }
        for (taxonomy_id, taxonomy, slug) in std::mem::take(&mut self.term_fixups) {
            if let Some((parent, _)) = self.find_term(&taxonomy, &slug)? {
                self.fixup_room(total)?;
                let sql = self.sql.update(
                    Table::Taxonomy,
                    &[(b"parent", Value::Int(parent))],
                    b"term_taxonomy_id",
                    taxonomy_id,
                )?;
                self.write(&sql)?;
                self.report.fixups += 1;
            }
        }
        if self.open {
            self.commit(total)?;
        } else if total > self.report.resumed_from {
            self.ports
                .checkpoint
                .save(total)
                .map_err(ImportError::Checkpoint)?;
        }
        // Every identity moves past the stored maximum, outside any batch, on every run: a run
        // that died between its last commit and this step leaves it to the resumed run.
        for table in Table::ALL {
            let max = match (self.ids.get(&table), self.sql.max(table)) {
                (_, None) => continue,
                (Some(ids), Some(_)) => ids.max,
                (None, Some(sql)) => self.first_id(&sql)?.unwrap_or(0),
            };
            if max == 0 {
                continue;
            }
            if let Some(sql) = self.sql.advance(table, max)? {
                self.query(&sql)?;
                self.report.max_ids.insert(table.stem(), max);
            }
        }
        self.report.records = total;
        Ok(self.report)
    }
}

/// The lower-cased host of `url` (between `://` and the next `/`).
fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// A login reduced to the characters the reference keeps in `user_nicename`.
fn nicename(login: &str) -> String {
    let mut out: String = login
        .chars()
        .map(|ch| {
            let ch = ch.to_ascii_lowercase();
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    out.truncate(50);
    out
}

#[cfg(test)]
mod tests {
    use super::{Options, host, nicename, rebuild_event};
    use cms_db::TablePrefix;
    use cms_outbox::{Aggregate, Change};

    #[test]
    fn helpers_cover_their_edges() {
        assert_eq!(host("https://Example.COM/blog"), "example.com");
        assert_eq!(host("example.com"), "example.com");
        assert_eq!(nicename("Ada Lovelace!"), "ada-lovelace-");
        assert_eq!(nicename(&"x".repeat(60)).len(), 50);
        let event = rebuild_event("site/imports/a.xml");
        assert_eq!(event.aggregate(), Aggregate::Site);
        assert_eq!(event.aggregate_id(), 0);
        assert_eq!(event.change(), Change::RebuildAll);
        assert_eq!(
            serde_json::Value::Object(event.payload().clone()).to_string(),
            "{\"import\":\"site/imports/a.xml\"}"
        );
        let prefix = TablePrefix::new("fp_");
        assert!(prefix.is_ok());
        if let Ok(prefix) = prefix {
            let mut options = Options::new(prefix, "k");
            assert_eq!(options.caps(), (2_500, 8 * 1024 * 1024));
            options.batch_rows = 10_000;
            options.batch_bytes = 0;
            assert_eq!(options.caps(), (2_500, 1));
        }
    }
}
