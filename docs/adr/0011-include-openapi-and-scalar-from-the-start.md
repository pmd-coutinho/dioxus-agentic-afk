# Include OpenAPI and Scalar from the start

The boilerplate will expose an OpenAPI document from the Axum API and serve a Scalar API reference for local inspection. OpenAPI is part of the initial control-plane foundation so API contracts are visible early, while Scalar is the documentation UI rather than the source of truth for the schema.

The OpenAPI document will be served at `/api/openapi.json`, and the Scalar API reference will be served at `/api/docs`.

**Considered Options**

- Defer OpenAPI until the API has consumers beyond the dashboard.
- Include OpenAPI generation and a Scalar reference from the start.
