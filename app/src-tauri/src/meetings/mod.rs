//! Meetings in the app: the real backend of the pipeline, the registry of the jobs that run it, the commands and views of
//! the meetings window, and the local server that plays a recording's media to it.

// Each part is wired into the app by a commit of its own; the commit that ends the series removes this.
#![allow(dead_code)]

pub mod media_server;
