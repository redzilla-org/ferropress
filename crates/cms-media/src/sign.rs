//! Upload signing: check the request, then presign a PUT into `site/incoming/`.

use std::fmt::Write as _;

use crate::name::{sanitize_file_name, split};
use crate::{Error, ImageKind, Presigner, Uploaders};

/// Signed metadata header carrying the uploader's user id to the processor.
pub const AUTHOR_HEADER: &str = "x-amz-meta-fp-author";

/// Site limits for uploads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignPolicy {
    /// Largest accepted upload in bytes.
    pub max_bytes: u64,
    /// Lifetime of a presigned URL in seconds.
    pub expires_secs: u32,
}

/// What the editor asks to upload.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SignRequest<'a> {
    /// The `fp_session` cookie value.
    pub session: &'a [u8],
    /// The client's file name.
    pub filename: &'a str,
    /// The declared mime type.
    pub mime: &'a str,
    /// The declared size in bytes.
    pub size: u64,
}

// The session token is a secret: never print it.
impl std::fmt::Debug for SignRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignRequest")
            .field("filename", &self.filename)
            .field("mime", &self.mime)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// A granted upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// The incoming object key.
    pub key: String,
    /// The presigned PUT URL.
    pub url: String,
    /// Headers the PUT must send unchanged (they are signed).
    pub headers: Vec<(String, String)>,
}

/// `site/incoming/<uuid v4>/<name>` for 16 random bytes and a sanitized name.
#[must_use]
pub fn incoming_key(random: [u8; 16], name: &str) -> String {
    let mut bytes = random;
    // RFC 9562 version 4 and variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut key = String::with_capacity(51 + name.len());
    key.push_str("site/incoming/");
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            key.push('-');
        }
        // Writing into a String cannot fail.
        let _ = write!(key, "{byte:02x}");
    }
    key.push('/');
    key.push_str(name);
    key
}

/// Checks the session, size, name and type, then presigns the PUT.
///
/// # Errors
///
/// [`Error::Forbidden`] without an uploader session, [`Error::Size`], [`Error::Name`] or
/// [`Error::FileType`] for a refused file, and [`Error::Port`] when an adapter fails.
pub fn sign(
    request: &SignRequest<'_>,
    policy: SignPolicy,
    random: [u8; 16],
    uploaders: &mut impl Uploaders,
    presigner: &impl Presigner,
) -> Result<Signed, Error> {
    // Authorization first, so an anonymous caller learns nothing about the policy.
    let author = uploaders
        .uploader(request.session)?
        .ok_or(Error::Forbidden)?;
    if request.size == 0 || request.size > policy.max_bytes {
        return Err(Error::Size {
            size: request.size,
            max: policy.max_bytes,
        });
    }
    let clean = sanitize_file_name(request.filename)?;
    let (stem, extension) = split(&clean);
    let kind = ImageKind::from_extension(extension).ok_or(Error::FileType)?;
    if !request.mime.eq_ignore_ascii_case(kind.mime()) {
        return Err(Error::FileType);
    }
    // The reference lower-cases the extension when it settles the stored name.
    let name = format!("{stem}.{}", extension.to_ascii_lowercase());
    let key = incoming_key(random, &name);
    let headers = vec![
        ("content-length".to_owned(), request.size.to_string()),
        ("content-type".to_owned(), kind.mime().to_owned()),
        (AUTHOR_HEADER.to_owned(), author.to_string()),
    ];
    let pairs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let url = presigner.presign_put(&key, &pairs, policy.expires_secs)?;
    Ok(Signed { key, url, headers })
}

#[cfg(test)]
mod tests {
    use super::{SignPolicy, SignRequest, incoming_key, sign};
    use crate::{Error, PortError, Presigner, Uploaders};
    use std::cell::RefCell;

    struct Sessions(Result<Option<u64>, PortError>);

    impl Uploaders for Sessions {
        fn uploader(&mut self, session: &[u8]) -> Result<Option<u64>, PortError> {
            assert_eq!(session, b"tok");
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct Signer {
        seen: RefCell<Vec<String>>,
        fail: bool,
    }

    impl Presigner for Signer {
        fn presign_put(
            &self,
            key: &str,
            headers: &[(&str, &str)],
            expires_secs: u32,
        ) -> Result<String, PortError> {
            if self.fail {
                return Err(PortError::new("no credentials"));
            }
            let names: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
            self.seen
                .borrow_mut()
                .push(format!("{key} {} {expires_secs}", names.join(";")));
            Ok(format!(
                "https://bucket.example.com/{key}?X-Amz-Signature=sig"
            ))
        }
    }

    const POLICY: SignPolicy = SignPolicy {
        max_bytes: 1000,
        expires_secs: 300,
    };

    fn request<'a>(filename: &'a str, mime: &'a str, size: u64) -> SignRequest<'a> {
        SignRequest {
            session: b"tok",
            filename,
            mime,
            size,
        }
    }

    #[test]
    fn keys_are_version_four_uuids() {
        assert_eq!(
            incoming_key([0xff; 16], "a.jpg"),
            "site/incoming/ffffffff-ffff-4fff-bfff-ffffffffffff/a.jpg"
        );
        assert_eq!(
            incoming_key([0; 16], "b.png"),
            "site/incoming/00000000-0000-4000-8000-000000000000/b.png"
        );
    }

    #[test]
    fn grants_a_sanitized_key_with_signed_headers() {
        let presigner = Signer::default();
        let signed = sign(
            &request("My Photo.JPG", "image/jpeg", 500),
            POLICY,
            [0; 16],
            &mut Sessions(Ok(Some(7))),
            &presigner,
        );
        let key = "site/incoming/00000000-0000-4000-8000-000000000000/My-Photo.jpg";
        assert_eq!(signed.as_ref().map(|s| s.key.as_str()), Ok(key));
        assert!(
            signed
                .as_ref()
                .is_ok_and(|s| s.url.ends_with("?X-Amz-Signature=sig"))
        );
        assert_eq!(
            signed.map(|s| s.headers),
            Ok(vec![
                ("content-length".into(), "500".into()),
                ("content-type".into(), "image/jpeg".into()),
                ("x-amz-meta-fp-author".into(), "7".into()),
            ])
        );
        assert_eq!(
            presigner.seen.borrow().as_slice(),
            [format!(
                "{key} content-length;content-type;x-amz-meta-fp-author 300"
            )]
        );
    }

    #[test]
    fn refuses_anonymous_oversized_and_foreign_files() {
        let presigner = Signer::default();
        let mut ok = Sessions(Ok(Some(1)));
        let refuse = |r: &SignRequest<'_>, s: &mut Sessions| {
            sign(r, POLICY, [1; 16], s, &presigner).map(|signed| signed.key)
        };
        assert_eq!(
            refuse(&request("a.jpg", "image/jpeg", 1), &mut Sessions(Ok(None))),
            Err(Error::Forbidden)
        );
        assert_eq!(
            refuse(
                &request("a.jpg", "image/jpeg", 1),
                &mut Sessions(Err(PortError::new("down")))
            ),
            Err(Error::Port(PortError::new("down")))
        );
        assert_eq!(
            refuse(&request("a.jpg", "image/jpeg", 0), &mut ok),
            Err(Error::Size { size: 0, max: 1000 })
        );
        assert_eq!(
            refuse(&request("a.jpg", "image/jpeg", 1001), &mut ok),
            Err(Error::Size {
                size: 1001,
                max: 1000
            })
        );
        assert_eq!(
            refuse(&request("a.pdf", "application/pdf", 5), &mut ok),
            Err(Error::FileType)
        );
        assert_eq!(
            refuse(&request("a.png", "image/jpeg", 5), &mut ok),
            Err(Error::FileType)
        );
        assert_eq!(
            refuse(&request("***", "image/png", 5), &mut ok),
            Err(Error::Name)
        );
        assert!(presigner.seen.borrow().is_empty());
        let failing = Signer {
            fail: true,
            ..Signer::default()
        };
        assert_eq!(
            sign(
                &request("a.gif", "IMAGE/GIF", 5),
                POLICY,
                [1; 16],
                &mut ok,
                &failing
            ),
            Err(Error::Port(PortError::new("no credentials")))
        );
    }

    #[test]
    fn debug_never_shows_the_session() {
        let shown = format!("{:?}", request("a.jpg", "image/jpeg", 1));
        assert!(shown.contains("a.jpg"));
        assert!(!shown.contains("tok"));
    }
}
