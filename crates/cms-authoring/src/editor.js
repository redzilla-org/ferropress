// Ferropress admin client. CloudFront Origin Access Control signs a POST or PUT to the admin
// function only when the browser sends the body's SHA-256 in x-amz-content-sha256, which a
// plain form submit cannot do, so signed forms are sent with fetch instead.
(function () {
  "use strict";

  // Hex SHA-256 of the exact body bytes, computed with WebCrypto.
  async function sha256Hex(text) {
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
    return Array.from(new Uint8Array(digest), function (b) {
      return b.toString(16).padStart(2, "0");
    }).join("");
  }

  // Sends any body with the hash header; REST callers use this for JSON.
  async function signedFetch(url, method, contentType, body) {
    return fetch(url, {
      method: method,
      credentials: "same-origin",
      headers: { "content-type": contentType, "x-amz-content-sha256": await sha256Hex(body) },
      body: body,
    });
  }

  // Posts the form (with the clicked button's name and value) and follows the 303 the admin
  // answers on success; any other answer replaces the page so its error is visible.
  async function submit(form, submitter) {
    const body = new URLSearchParams(new FormData(form, submitter)).toString();
    const response = await signedFetch(
      form.action, "POST", "application/x-www-form-urlencoded", body);
    if (response.redirected) {
      window.location.assign(response.url);
      return;
    }
    document.documentElement.innerHTML = await response.text();
  }

  document.addEventListener("submit", function (event) {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !form.hasAttribute("data-fp-signed")) {
      return;
    }
    event.preventDefault();
    submit(form, event.submitter).catch(function (error) {
      window.alert("Saving failed: " + error.message);
    });
  });

  window.fpAdmin = { sha256Hex: sha256Hex, signedFetch: signedFetch };
})();
