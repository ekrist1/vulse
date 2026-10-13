-- Cells are always read by record. The value index let the query planner
-- scan every cell of a field across all collections for each record, which
-- made sorting and filtering by text slow in large workspaces.
DROP INDEX IF EXISTS idx_document_cells_values;
