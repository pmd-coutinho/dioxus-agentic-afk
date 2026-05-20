# Use RFC 7807 problem+json for API errors

The API will represent errors using RFC 7807 problem details with the `application/problem+json` media type. This avoids inventing a custom error envelope and gives the dashboard, OpenAPI document, and tests a standard error shape from the start.

**Considered Options**

- Use a custom `{ error: { code, message } }` envelope.
- Use RFC 7807 problem details.
