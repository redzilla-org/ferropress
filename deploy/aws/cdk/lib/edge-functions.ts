// CloudFront Functions (cloudfront-js-2.0) source, built as strings so the values that differ
// per site (base path, search origin domain) are embedded at synth. Kept free of CDK imports
// so tests can execute the generated handlers directly.

/**
 * Viewer-request router for the public behaviors:
 * - the bare base path without its trailing slash (`/blog`) answers 301 to `/blog/`;
 * - any request with query `s` is sent to the search function URL (signed by OAC).
 *
 * `searchDomain` may be a CDK token; it is spliced into the source as a JSON string literal.
 */
export function routerCode(basePath: string, searchDomain: string): string {
  const bare = basePath === '/' ? null : basePath.slice(0, -1);
  return [
    "import cf from 'cloudfront';",
    `var BASE = ${JSON.stringify(basePath)};`,
    `var BARE = ${JSON.stringify(bare)};`,
    `var SEARCH = "${searchDomain}";`,
    // Rebuild the query string so the redirect keeps it.
    'function query(qs) {',
    '  var parts = [];',
    '  for (var key in qs) {',
    '    var entry = qs[key];',
    '    var values = entry.multiValue ? entry.multiValue : [entry];',
    '    for (var i = 0; i < values.length; i++) {',
    "      parts.push(values[i].value === '' ? key : key + '=' + values[i].value);",
    '    }',
    '  }',
    "  return parts.length ? '?' + parts.join('&') : '';",
    '}',
    'function handler(event) {',
    '  var request = event.request;',
    '  if (BARE !== null && request.uri === BARE) {',
    '    return {',
    '      statusCode: 301,',
    "      statusDescription: 'Moved Permanently',",
    '      headers: { location: { value: BASE + query(request.querystring) } },',
    '    };',
    '  }',
    "  if (Object.prototype.hasOwnProperty.call(request.querystring, 's')) {",
    '    cf.updateRequestOrigin({',
    '      domainName: SEARCH,',
    "      originAccessControlConfig: { enabled: true, signingBehavior: 'always', signingProtocol: 'sigv4', originType: 'lambda' },",
    '    });',
    '  }',
    '  return request;',
    '}',
  ].join('\n');
}

/**
 * Viewer-request rewrite for media: `<base>wp-content/uploads/<rest>` is stored at
 * `site/uploads/<rest>`; the S3 origin path supplies `/site`, so the URI becomes `/uploads/<rest>`.
 */
export function uploadsRewriteCode(basePath: string): string {
  const prefix = `${basePath}wp-content/uploads/`;
  return [
    `var PREFIX = ${JSON.stringify(prefix)};`,
    'function handler(event) {',
    '  var request = event.request;',
    '  if (request.uri.indexOf(PREFIX) === 0) {',
    "    request.uri = '/uploads/' + request.uri.slice(PREFIX.length);",
    '  }',
    '  return request;',
    '}',
  ].join('\n');
}
