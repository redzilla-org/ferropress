//! `CloudFront` invalidations (REST/XML API `2020-05-31`).

use crate::http::{Call, xml_text};
use crate::sigv4::PathStyle;
use crate::{AwsClient, AwsError, Endpoint};

/// A `CloudFront` client.
#[derive(Clone, Debug)]
pub struct CloudFront {
    aws: AwsClient,
    endpoint: Endpoint,
}

impl CloudFront {
    /// A client reaching `endpoint`.
    #[must_use]
    pub fn new(aws: AwsClient, endpoint: Endpoint) -> Self {
        Self { aws, endpoint }
    }

    /// The global endpoint `https://cloudfront.amazonaws.com`.
    #[must_use]
    pub fn global_endpoint() -> Endpoint {
        Endpoint::https("cloudfront.amazonaws.com")
    }

    /// `CreateInvalidation` for `paths` (each starting with `/`, `*` allowed at the end) on
    /// `distribution_id`. `caller_reference` makes retries idempotent: reuse it for the same
    /// batch. Returns the invalidation id.
    ///
    /// # Errors
    ///
    /// [`AwsError::Input`] for an empty batch, otherwise [`AwsError`] when the call fails or
    /// the reply has no id.
    pub fn create_invalidation(
        &self,
        distribution_id: &str,
        paths: &[&str],
        caller_reference: &str,
    ) -> Result<Box<str>, AwsError> {
        const OP: &str = "CloudFront.CreateInvalidation";
        if paths.is_empty() {
            return Err(AwsError::Input {
                operation: OP,
                what: "no paths",
            });
        }
        let mut body = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <InvalidationBatch xmlns=\"http://cloudfront.amazonaws.com/doc/2020-05-31/\">",
        );
        body.push_str("<Paths><Quantity>");
        body.push_str(&paths.len().to_string());
        body.push_str("</Quantity><Items>");
        for path in paths {
            body.push_str("<Path>");
            body.push_str(&escape(path));
            body.push_str("</Path>");
        }
        body.push_str("</Items></Paths><CallerReference>");
        body.push_str(&escape(caller_reference));
        body.push_str("</CallerReference></InvalidationBatch>");
        let path = format!(
            "/2020-05-31/distribution/{}/invalidation",
            crate::sigv4::uri_encode(distribution_id, false)
        );
        // CloudFront is global and signs in us-east-1 whatever the caller's region.
        let reply = self
            .aws
            .send(&Call {
                operation: OP,
                service: "cloudfront",
                region: Some("us-east-1"),
                path_style: PathStyle::Standard,
                endpoint: &self.endpoint,
                method: "POST",
                path: &path,
                query: &[],
                headers: &[("content-type", "text/xml")],
                body: body.as_bytes(),
            })?
            .success(OP)?;
        let text = std::str::from_utf8(&reply.body).unwrap_or_default();
        xml_text(text, "Id")
            .filter(|id| !id.is_empty())
            .map(Into::into)
            .ok_or(AwsError::Malformed {
                operation: OP,
                what: "no invalidation Id",
            })
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape;

    #[test]
    fn xml_text_is_escaped() {
        assert_eq!(escape("/a?b=1&c=<2>\""), "/a?b=1&amp;c=&lt;2&gt;&quot;");
    }
}
