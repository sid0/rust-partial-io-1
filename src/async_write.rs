/*
 *  Copyright (c) 2017-present, Facebook, Inc.
 *  All rights reserved.
 *
 *  This source code is licensed under the BSD-style license found in the
 *  LICENSE file in the root directory of this source tree. An additional grant
 *  of patent rights can be found in the PATENTS file in the same directory.
 *
 */

//! This module contains an `AsyncWrite` wrapper that breaks writes up
//! according to a provided iterator.
//!
//! This is separate from `PartialWrite` because on `WouldBlock` errors, it
//! causes `futures` to try writing or flushing again.

use std::cmp;
use std::fmt;
use std::io::{self, Read, Write};

use futures::{task, Poll};
use tokio_io::{AsyncRead, AsyncWrite};

use crate::{make_ops, PartialOp};

/// A wrapper that breaks inner `AsyncWrite` instances up according to the
/// provided iterator.
///
/// Available with the `tokio` feature.
///
/// # Examples
///
/// ```rust
/// extern crate partial_io;
/// extern crate tokio_core;
/// extern crate tokio_io;
///
/// use std::io::{self, Cursor};
///
/// fn main() {
///     // Note that this test doesn't demonstrate a limited write because
///     // tokio-io doesn't have a combinator for that, just write_all.
///     use tokio_core::reactor::Core;
///     use tokio_io::io::write_all;
///
///     use partial_io::{PartialAsyncWrite, PartialOp};
///
///     let writer = Cursor::new(Vec::new());
///     let iter = vec![PartialOp::Err(io::ErrorKind::WouldBlock), PartialOp::Limited(2)];
///     let partial_writer = PartialAsyncWrite::new(writer, iter);
///     let in_data = vec![1, 2, 3, 4];
///
///     let mut core = Core::new().unwrap();
///
///     let write_fut = write_all(partial_writer, in_data);
///
///     let (partial_writer, _in_data) = core.run(write_fut).unwrap();
///     let cursor = partial_writer.into_inner();
///     let out = cursor.into_inner();
///     assert_eq!(&out, &[1, 2, 3, 4]);
/// }
/// ```
pub struct PartialAsyncWrite<W> {
    inner: W,
    ops: Box<dyn Iterator<Item = PartialOp> + Send>,
}

impl<W> PartialAsyncWrite<W>
where
    W: AsyncWrite,
{
    /// Creates a new `PartialAsyncWrite` wrapper over the writer with the specified `PartialOp`s.
    pub fn new<I>(inner: W, iter: I) -> Self
    where
        I: IntoIterator<Item = PartialOp> + 'static,
        I::IntoIter: Send,
    {
        PartialAsyncWrite {
            inner,
            ops: make_ops(iter),
        }
    }

    /// Sets the `PartialOp`s for this reader.
    pub fn set_ops<I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = PartialOp> + 'static,
        I::IntoIter: Send,
    {
        self.ops = make_ops(iter);
        self
    }

    /// Acquires a mutable reference to the underlying writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Consumes this wrapper, returning the underlying writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W> Write for PartialAsyncWrite<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.ops.next() {
            Some(PartialOp::Limited(n)) => {
                let len = cmp::min(n, buf.len());
                self.inner.write(&buf[..len])
            }
            Some(PartialOp::Err(err)) => {
                if err == io::ErrorKind::WouldBlock {
                    // Make sure this task is rechecked.
                    task::park().unpark();
                }
                Err(io::Error::new(
                    err,
                    "error during write, generated by partial-io",
                ))
            }
            Some(PartialOp::Unlimited) | None => self.inner.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.ops.next() {
            Some(PartialOp::Err(err)) => {
                if err == io::ErrorKind::WouldBlock {
                    // Make sure this task is rechecked.
                    task::park().unpark();
                }
                Err(io::Error::new(
                    err,
                    "error during flush, generated by partial-io",
                ))
            }
            _ => self.inner.flush(),
        }
    }
}

impl<W> AsyncWrite for PartialAsyncWrite<W>
where
    W: AsyncWrite,
{
    #[inline]
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        self.inner.shutdown()
    }
}

// Forwarding impls to support duplex structs.
impl<W> Read for PartialAsyncWrite<W>
where
    W: AsyncWrite + Read,
{
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<W> AsyncRead for PartialAsyncWrite<W> where W: AsyncRead + AsyncWrite {}

impl<W> fmt::Debug for PartialAsyncWrite<W>
where
    W: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartialAsyncWrite")
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::File;

    use crate::tests::assert_send;

    #[test]
    fn test_sendable() {
        assert_send::<PartialAsyncWrite<File>>();
    }
}
