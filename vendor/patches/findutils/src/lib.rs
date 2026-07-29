// Copyright 2017 Google Inc.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

pub mod find;
pub mod locate;
pub mod updatedb;
pub mod xargs;

// YourShell patch: pluggable exec seam so `find -exec` / `xargs` work on
// platforms that forbid fork/exec (iOS). See exec_hook.rs.
pub mod exec_hook;
