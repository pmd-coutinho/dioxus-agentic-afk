# Use SQLx for SQLite persistence

The persistence crate will use SQLx for SQLite access and migrations. SQLx fits the async Axum server, supports SQLite, provides migration tooling, and allows compile-time checked queries where practical without introducing a heavier ORM abstraction.

**Considered Options**

- Use Diesel or another ORM.
- Use raw SQLite bindings directly.
- Use SQLx with SQLite and migrations.
