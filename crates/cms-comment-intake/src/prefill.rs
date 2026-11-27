//! The tiny browser script that keeps public pages cookie-free.
//!
//! Pages are cached for everyone, so the server never personalizes them. This script fills the
//! comment form from the commenter cookies set by the comment endpoint, and submits the form
//! with the body's SHA-256 in `x-amz-content-sha256`, which `CloudFront` origin access control
//! needs to sign a `POST` to a function URL. The endpoint answers such a submission with JSON
//! (`location` or `error`) instead of a redirect, because a script cannot read a redirect's
//! `Location`.

/// Header the script sends so the endpoint answers with JSON.
pub const FETCH_HEADER: &str = "x-fp-submit";

/// The script, to be served as a static asset next to the rendered pages.
pub const PREFILL_SCRIPT: &str = r#"(function () {
  "use strict";
  var form = document.getElementById("commentform");
  if (!form || !window.crypto || !crypto.subtle) { return; }
  // Commenter cookies end in a 32-hex site hash; email_ and url_ name the other two fields.
  var fields = { "": "author", "email_": "email", "url_": "url" };
  var found = false;
  document.cookie.split("; ").forEach(function (pair) {
    var match = /^comment_author_(email_|url_)?[0-9a-f]{32}=(.*)$/.exec(pair);
    if (!match) { return; }
    var input = document.getElementById(fields[match[1] || ""]);
    var value = decodeURIComponent(match[2].replace(/\+/g, " "));
    if (input && !input.value && value) { input.value = value; found = true; }
  });
  var consent = document.getElementById("wp-comment-cookies-consent");
  if (consent && found) { consent.checked = true; }
  form.addEventListener("submit", function (event) {
    event.preventDefault();
    var body = new URLSearchParams(new FormData(form)).toString();
    crypto.subtle.digest("SHA-256", new TextEncoder().encode(body)).then(function (digest) {
      var hex = Array.prototype.map.call(new Uint8Array(digest), function (b) {
        return ("0" + b.toString(16)).slice(-2);
      }).join("");
      return fetch(form.action, {
        method: "POST",
        body: body,
        credentials: "same-origin",
        headers: {
          "content-type": "application/x-www-form-urlencoded",
          "x-amz-content-sha256": hex,
          "x-fp-submit": "fetch"
        }
      });
    }).then(function (response) {
      return response.json();
    }).then(function (reply) {
      if (reply.location) { window.location.assign(reply.location); return; }
      var note = document.getElementById("comment-submit-error") || document.createElement("p");
      note.id = "comment-submit-error";
      note.setAttribute("role", "alert");
      note.innerHTML = reply.error || "";
      form.parentNode.insertBefore(note, form);
    });
  });
})();
"#;

#[cfg(test)]
mod tests {
    use super::{FETCH_HEADER, PREFILL_SCRIPT};

    #[test]
    fn the_script_reads_the_cookie_names_and_sends_the_headers() {
        assert!(PREFILL_SCRIPT.contains("comment_author_(email_|url_)?[0-9a-f]{32}"));
        assert!(PREFILL_SCRIPT.contains("x-amz-content-sha256"));
        assert!(PREFILL_SCRIPT.contains(FETCH_HEADER));
        assert!(PREFILL_SCRIPT.len() < 4096);
    }
}
