/// To run this benchmark:
///
/// > DENO_BUILD_MODE=release ./tools/build.py && \
///   ./target/release/deno_core_http_bench --multi-thread
extern crate deno;
extern crate futures;
extern crate libc;
extern crate tokio;

#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

use deno::*;
use futures::future::FutureExt;
use futures::future::TryFutureExt;
use std::env;
use std::future::Future;
use std::io::Error;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::task::Poll;
use tokio::prelude::Async;
use tokio::prelude::AsyncRead;
use tokio::prelude::AsyncWrite;

static LOGGER: Logger = Logger;
struct Logger;
impl log::Log for Logger {
  fn enabled(&self, metadata: &log::Metadata) -> bool {
    metadata.level() <= log::max_level()
  }
  fn log(&self, record: &log::Record) {
    if self.enabled(record.metadata()) {
      println!("{} - {}", record.level(), record.args());
    }
  }
  fn flush(&self) {}
}

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
  pub promise_id: i32,
  pub arg: i32,
  pub result: i32,
}

impl Into<Buf> for Record {
  fn into(self) -> Buf {
    let buf32 = vec![self.promise_id, self.arg, self.result].into_boxed_slice();
    let ptr = Box::into_raw(buf32) as *mut [u8; 3 * 4];
    unsafe { Box::from_raw(ptr) }
  }
}

impl From<&[u8]> for Record {
  fn from(s: &[u8]) -> Record {
    #[allow(clippy::cast_ptr_alignment)]
    let ptr = s.as_ptr() as *const i32;
    let ints = unsafe { std::slice::from_raw_parts(ptr, 3) };
    Record {
      promise_id: ints[0],
      arg: ints[1],
      result: ints[2],
    }
  }
}

impl From<Buf> for Record {
  fn from(buf: Buf) -> Record {
    assert_eq!(buf.len(), 3 * 4);
    #[allow(clippy::cast_ptr_alignment)]
    let ptr = Box::into_raw(buf) as *mut [i32; 3];
    let ints: Box<[i32]> = unsafe { Box::from_raw(ptr) };
    assert_eq!(ints.len(), 3);
    Record {
      promise_id: ints[0],
      arg: ints[1],
      result: ints[2],
    }
  }
}

#[test]
fn test_record_from() {
  let r = Record {
    promise_id: 1,
    arg: 3,
    result: 4,
  };
  let expected = r.clone();
  let buf: Buf = r.into();
  #[cfg(target_endian = "little")]
  assert_eq!(
    buf,
    vec![1u8, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0].into_boxed_slice()
  );
  let actual = Record::from(buf);
  assert_eq!(actual, expected);
  // TODO test From<&[u8]> for Record
}

pub type HttpOp = dyn Future<Output = Result<i32, std::io::Error>> + Send;

pub type HttpOpHandler =
  fn(record: Record, zero_copy_buf: Option<PinnedBuf>) -> Pin<Box<HttpOp>>;

fn http_op(
  handler: HttpOpHandler,
) -> impl Fn(&[u8], Option<PinnedBuf>) -> CoreOp {
  move |control: &[u8], zero_copy_buf: Option<PinnedBuf>| -> CoreOp {
    let record = Record::from(control);
    let is_sync = record.promise_id == 0;
    let op = handler(record.clone(), zero_copy_buf);

    let mut record_a = record.clone();
    let mut record_b = record.clone();

    let fut = Box::new(
      op.and_then(move |result| {
        record_a.result = result;
        futures::future::ok(record_a)
      })
      .or_else(|err| {
        eprintln!("unexpected err {}", err);
        record_b.result = -1;
        futures::future::ok(record_b)
      })
      .then(|result: Result<Record, ()>| {
        let record = result.unwrap();
        futures::future::ok(record.into())
      }),
    );

    if is_sync {
      Op::Sync(futures::executor::block_on(fut).unwrap())
    } else {
      Op::Async(fut.boxed())
    }
  }
}

fn main() {
  let args: Vec<String> = env::args().collect();
  // NOTE: `--help` arg will display V8 help and exit
  let args = deno::v8_set_flags(args);

  log::set_logger(&LOGGER).unwrap();
  log::set_max_level(if args.iter().any(|a| a == "-D") {
    log::LevelFilter::Debug
  } else {
    log::LevelFilter::Warn
  });

  let js_source = include_str!("http_bench.js");

  let startup_data = StartupData::Script(Script {
    source: js_source,
    filename: "http_bench.js",
  });

  let mut isolate = deno::Isolate::new(startup_data, false);
  isolate.register_op("listen", http_op(op_listen));
  isolate.register_op("accept", http_op(op_accept));
  isolate.register_op("read", http_op(op_read));
  isolate.register_op("write", http_op(op_write));
  isolate.register_op("close", http_op(op_close));

  let main_future = isolate
    .then(|r| {
      js_check(r);
      futures::future::ok(())
    })
    .boxed();

  if args.iter().any(|a| a == "--multi-thread") {
    println!("multi-thread");
    tokio::run(main_future.compat());
  } else {
    println!("single-thread");
    tokio::runtime::current_thread::run(main_future.compat());
  }
}

pub fn bad_resource() -> Error {
  Error::new(ErrorKind::NotFound, "bad resource id")
}

struct TcpListener(tokio::net::TcpListener);

impl Resource for TcpListener {}

struct TcpStream(tokio::net::TcpStream);

impl Resource for TcpStream {}

lazy_static! {
  static ref RESOURCE_TABLE: Mutex<ResourceTable> =
    Mutex::new(ResourceTable::default());
}

fn lock_resource_table<'a>() -> MutexGuard<'a, ResourceTable> {
  RESOURCE_TABLE.lock().unwrap()
}

fn op_accept(
  record: Record,
  _zero_copy_buf: Option<PinnedBuf>,
) -> Pin<Box<HttpOp>> {
  let rid = record.arg as u32;
  debug!("accept {}", rid);
  let fut = futures::future::poll_fn(move |_cx| {
    let mut table = lock_resource_table();
    let listener =
      table.get_mut::<TcpListener>(rid).ok_or_else(bad_resource)?;
    match listener.0.poll_accept() {
      Err(e) => Poll::Ready(Err(e)),
      Ok(Async::Ready(v)) => Poll::Ready(Ok(v)),
      Ok(Async::NotReady) => Poll::Pending,
    }
  })
  .and_then(move |(stream, addr)| {
    debug!("accept success {}", addr);
    let mut table = lock_resource_table();
    let rid = table.add("tcpStream", Box::new(TcpStream(stream)));
    futures::future::ok(rid as i32)
  });
  fut.boxed()
}

fn op_listen(
  _record: Record,
  _zero_copy_buf: Option<PinnedBuf>,
) -> Pin<Box<HttpOp>> {
  debug!("listen");
  let addr = "127.0.0.1:4544".parse::<SocketAddr>().unwrap();
  let listener = tokio::net::TcpListener::bind(&addr).unwrap();
  let mut table = lock_resource_table();
  let rid = table.add("tcpListener", Box::new(TcpListener(listener)));
  futures::future::ok(rid as i32).boxed()
}

fn op_close(
  record: Record,
  _zero_copy_buf: Option<PinnedBuf>,
) -> Pin<Box<HttpOp>> {
  debug!("close");
  let rid = record.arg as u32;
  let mut table = lock_resource_table();
  let fut = match table.close(rid) {
    Some(_) => futures::future::ok(0),
    None => futures::future::err(bad_resource()),
  };
  fut.boxed()
}

fn op_read(
  record: Record,
  zero_copy_buf: Option<PinnedBuf>,
) -> Pin<Box<HttpOp>> {
  let rid = record.arg as u32;
  debug!("read rid={}", rid);
  let mut zero_copy_buf = zero_copy_buf.unwrap();
  let fut = futures::future::poll_fn(move |_cx| {
    let mut table = lock_resource_table();
    let stream = table.get_mut::<TcpStream>(rid).ok_or_else(bad_resource)?;
    match stream.0.poll_read(&mut zero_copy_buf) {
      Err(e) => Poll::Ready(Err(e)),
      Ok(Async::Ready(v)) => Poll::Ready(Ok(v)),
      Ok(Async::NotReady) => Poll::Pending,
    }
  })
  .and_then(move |nread| {
    debug!("read success {}", nread);
    futures::future::ok(nread as i32)
  });
  fut.boxed()
}

fn op_write(
  record: Record,
  zero_copy_buf: Option<PinnedBuf>,
) -> Pin<Box<HttpOp>> {
  let rid = record.arg as u32;
  debug!("write rid={}", rid);
  let zero_copy_buf = zero_copy_buf.unwrap();
  let fut = futures::future::poll_fn(move |_cx| {
    let mut table = lock_resource_table();
    let stream = table.get_mut::<TcpStream>(rid).ok_or_else(bad_resource)?;
    match stream.0.poll_write(&zero_copy_buf) {
      Err(e) => Poll::Ready(Err(e)),
      Ok(Async::Ready(v)) => Poll::Ready(Ok(v)),
      Ok(Async::NotReady) => Poll::Pending,
    }
  })
  .and_then(move |nwritten| {
    debug!("write success {}", nwritten);
    futures::future::ok(nwritten as i32)
  });
  fut.boxed()
}

fn js_check(r: Result<(), ErrBox>) {
  if let Err(e) = r {
    panic!(e.to_string());
  }
}
