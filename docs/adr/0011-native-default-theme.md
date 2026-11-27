# ADR-0011: Native default theme for the serverless profile

- **Status:** Proposed. This is the owner's default ruling for the AWS Lambda port (issue #6). It
  becomes Accepted when the projector serves the imported blog's routes from `cms-render` output.
- **Date:** 2026-11-27

## Context

The serverless profile (ADR-0010, `docs/aws/architecture.md` section 5) pre-renders every public
route into S3. HTML pages need a renderer. Roadmap steps 24–25 (themes, blocks, formatting) have
not started, and the projector cannot run PHP theme templates: the read and projector Lambdas
link no PHP engine. Without some renderer, the profile can serve feeds and sitemaps but no pages.

## Decision

- **`cms-render`** is the built-in theme of the serverless profile. It is a domain crate of pure
  functions over view models (`Site`, `Post`, `Archive`). It renders:
  - the shared shell;
  - page fragments for single posts, pages, the paged home, category, tag, author and date
    archives, and the 404 page;
  - RSS 2.0 feeds;
  - sitemaps at the reference's core paths (2,000 URLs per shard);
  - `robots.txt`.
- **Page composition.** The shell holds `<!--fp:head-->` and `<!--fp:body-->`. A fragment is its
  per-page head part, then `<!--fp:/head-->`, then the body. `cms_readmodel::assemble` composes the
  two, in the read Lambda and in the projector, so each ETag is computed over the bytes served.
- **Formatting.** Block delimiter comments are stripped from block-serialized content, and classic
  content is auto-paragraphed. Both were studied from reference behavior and implemented
  independently. Every non-content field is escaped.
- **Listing cards.** `[[<5 digits>]]` and `[[<listing id>]]` markers are resolved through the
  `CardSource` port. A marker with no card renders as nothing and is reported. A source error
  fails the render, so the projector retries.

## Consequences

- **PHP theme execution is a documented divergence of the serverless profile.** Themes, template
  hierarchy, theme hooks, widgets, menus beyond a flat link list, and dynamic blocks are not
  rendered there. The MySQL parity profile still targets unmodified PHP themes (master plan).
- The core sitemaps' XSL stylesheet line is not emitted. Feeds omit `lastBuildDate` when they have
  no items (the renderer has no clock).
- Excerpts never contain card text. The reference runs content filters before trimming an
  excerpt; this theme strips markers instead.
- Certification for themes is reported per profile, as ADR-0010 does for storage.
