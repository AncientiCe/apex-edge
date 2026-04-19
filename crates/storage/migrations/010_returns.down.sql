-- Rollback for migration 010_returns.sql
--
-- Drops the returns / return_lines / refunds tables. Because SQLite FKs default
-- off and we don't cascade from `orders`, no other migration depends on these
-- tables. Safe to replay forward: migration 010 is pure CREATE TABLE.

DROP TABLE IF EXISTS refunds;
DROP TABLE IF EXISTS return_lines;
DROP TABLE IF EXISTS returns;
