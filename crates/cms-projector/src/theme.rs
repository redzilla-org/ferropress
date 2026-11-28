//! The narrow rendering port, so the projector does not name one theme.

use cms_render::{
    Archive, CardError, CardSource, Channel, Output, Post, Rendered, Site, SitemapFile, SitemapUrls,
};

/// Turns view models into [`Rendered`] bytes. Each method defaults to the native built-in theme
/// (`cms-render`, ADR-0011), so [`Native`] is the whole production implementation.
pub trait Renderer {
    /// The shared page shell.
    fn shell(&self, site: &Site) -> Rendered {
        cms_render::shell(site)
    }

    /// A single post or page fragment.
    ///
    /// # Errors
    ///
    /// [`CardError`] when the card source fails.
    fn single(
        &self,
        site: &Site,
        post: &Post,
        cards: &mut dyn CardSource,
    ) -> Result<Output, CardError> {
        cms_render::single(site, post, cards)
    }

    /// One archive page fragment.
    fn archive(&self, site: &Site, page: &Archive<'_>) -> Rendered {
        cms_render::archive(site, page)
    }

    /// The not-found fragment.
    fn not_found(&self, site: &Site) -> Rendered {
        cms_render::not_found(site)
    }

    /// An RSS feed.
    ///
    /// # Errors
    ///
    /// [`CardError`] when the card source fails.
    fn feed(
        &self,
        site: &Site,
        channel: &Channel<'_>,
        cards: &mut dyn CardSource,
    ) -> Result<Output, CardError> {
        cms_render::feed(site, channel, cards)
    }

    /// The sitemap index first, then every shard.
    fn sitemaps(&self, site: &Site, urls: &SitemapUrls) -> Vec<SitemapFile> {
        cms_render::sitemaps(site, urls)
    }

    /// `robots.txt`.
    fn robots(&self, site: &Site) -> Rendered {
        cms_render::robots(site)
    }
}

/// The native built-in theme.
#[derive(Clone, Copy, Debug, Default)]
pub struct Native;

impl Renderer for Native {}
