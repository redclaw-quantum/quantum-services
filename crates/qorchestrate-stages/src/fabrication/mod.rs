//! Fabrication-handoff stages: turn a validated design into a GDS-II layout and
//! run a design-rule check over it, so a `design-to-chip` run reaches a
//! fabrication-ready artifact rather than stopping at an OQFP spec.
pub mod drc_check;
pub mod gds_generate;
pub mod process_recipe;
pub mod tapeout;
