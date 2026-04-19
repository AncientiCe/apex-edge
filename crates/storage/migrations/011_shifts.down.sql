-- Rollback for migration 011_shifts.sql
--
-- Drops the till/shift tables. No other migration depends on these tables.

DROP TABLE IF EXISTS shift_cash_counts;
DROP TABLE IF EXISTS shift_movements;
DROP TABLE IF EXISTS shifts;
