/*
 * Copyright 2026 Ankit Chaubey <ankitchaubey.dev@gmail.com>
 * github.com/ankit-chaubey
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use anyhow::Result;
use std::io::{self, ErrorKind};
use std::path::Path;

use crate::checksum::HashingReader;
use crate::index::ArchivumIndex;
use crate::restore::{check_stream, with_member};

pub fn cat(index_path: &Path, file: &Path) -> Result<()> {
    let idx = ArchivumIndex::read(index_path)?;
    let index_dir = index_path.parent().unwrap_or(Path::new("."));

    with_member(&idx, index_dir, file, |reader, origin| {
        let mut stdout = io::stdout().lock();
        let mut hr = HashingReader::new(reader);
        match io::copy(&mut hr, &mut stdout) {
            Ok(_) => {}
            // `archivum cat ... | head` closes the pipe early; that is not an error
            Err(e) if e.kind() == ErrorKind::BrokenPipe => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        let (hash, bytes) = hr.finalize();
        check_stream(origin, &hash, bytes)
    })
}
