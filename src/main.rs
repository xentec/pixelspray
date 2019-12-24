

use std::{
	io, io::prelude::*,
	net::TcpStream,
	path::PathBuf,
};

use clap::{arg_enum};
use structopt::{StructOpt};

use log::{info, warn};

use image::{Pixel, GenericImageView};
use rand::{seq::SliceRandom};

use r2d2;

arg_enum!{
	#[derive(Debug,PartialEq)]
	enum Filter
	{
		Mask,
		Grey,
		RGBA,
	}
}

#[derive(StructOpt, Debug)]
struct Opt
{
	#[structopt(help = "The host to connect to")]
	host: String,
	#[structopt(short = "f", long = "filter", help = "filter to use", default_value="RGBA")]
	filter: Filter,
	#[structopt(long = "filter-color", help = "one-color", default_value="255")]
	color: u8,
	#[structopt(help = "Image to spray", parse(from_os_str))]
	image: PathBuf,
	#[structopt(long = "no-offset", help = "disable offset")]
	no_offset: bool,
}

fn main()
{
	let opt = Opt::from_args();

	info!("pixelspray: {:?}", &opt);

	let image = image::open(&opt.image).expect("failed to load image");

	//image = image.resize(256, 256, image::FilterType::Nearest);
	//image = image.grayscale();

	let (sw,sh) = (1920, 1080);
//	let (sw,sh) = (1024, 1024);
	let (w,h) = image.dimensions();

	let (xoff,yoff) = ((sw-w), sh-h);
	let n = 8;

	info!("screen: {}x{} image: {}x{} offset: {}x{}", sw, sh, w, h, xoff, yoff);

	let mut pxs = image.pixels()
		.filter_map(|(mut x,mut y,p)|
		{
			let ch = p.channels().len();
			let (r,g,b,a) = p.channels4();

			if a < 5 { return None; }

			if opt.no_offset {
				x += xoff;
				y += yoff;
			}

			let px = match opt.filter
			{
				Filter::Mask => format!("PX {} {} {:02X}\n", x, y, opt.color),
				Filter::Grey => format!("PX {} {} {:02X}\n", x, y, r),
				Filter::RGBA if ch == 3 => format!("PX {} {} {:02X}{:02X}{:02X}\n", x, y, r, g, b),
				Filter::RGBA => format!("PX {} {} {:02X}{:02X}{:02X}{:02X}\n", x, y, r, g, b, a),
			};
			Some(px)
		}).collect::<Vec<_>>();

	pxs.shuffle(&mut rand::thread_rng());

	let mut bufs = Vec::new();
	{
		let mut buf = String::new();
		for px in pxs
		{
			buf.push_str(&px);
			if buf.len() > 1400
			{
				bufs.push(buf.clone());
				buf.clear();
			}
		}
		bufs.push(buf);
	}

	let c_size = bufs.len()/n+1;

	info!("chunks: {} a {} pxs", bufs.len(), bufs.len()/n+1);
	info!("Connecting to {}...", &opt.host);

	let manager = DataStreamManager::new(opt.host,
		if opt.no_offset { (0,0) } else { (xoff, yoff) });
	let pool = r2d2::Pool::builder()
		.max_size(15)
		.build(manager)
		.unwrap();

	use std::thread;

	let mut threads = Vec::new();
	for buf in bufs.chunks(c_size)
	{
		let pool = pool.clone();
		let c = buf.clone();
		let child = thread::spawn(move ||
			loop {
				let mut stream = pool.get().expect("no con");
				for b in c {
					stream.write_all(b.as_bytes()).ok();
				}
			});
		threads.push(child);
	};

	for t in threads
	{
		t.join().ok();
	}
}

#[derive(Debug, Clone)]
struct DataStreamManager
{
	host: String,
	offset: (u32, u32),
}

impl DataStreamManager {
	pub fn new(host: String, offset: (u32, u32)) -> Self
	{
		DataStreamManager { host: host, offset }
	}
}

#[derive(Debug)]
struct DataStream {
	inner: TcpStream,
	fail: Option<String>,
}

impl io::Write for DataStream {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize>
	{
		let res = self.inner.write(buf);
		self.fail = res.as_ref().map_err(|e| e.to_string()).err();
		res
	}

	fn flush(&mut self) -> io::Result<()>
	{
		let res = self.inner.flush();
		self.fail = res.as_ref().map_err(|e| e.to_string()).err();
		res
	}
}

impl r2d2::ManageConnection for DataStreamManager {
	/// The connection type this manager deals with.
	type Connection = DataStream;

	/// The error type returned by `Connection`s.
	type Error = std::io::Error;

	/// Attempts to create a new connection.
	fn connect(&self) -> Result<Self::Connection, Self::Error>
	{
		info!("connecting... offset: {}x{}", self.offset.0, self.offset.1);
		TcpStream::connect(&self.host)
			.and_then(|s| s.set_nodelay(true).map(|_| s))
			.and_then(|mut s| s.write_all(format!("OFFSET {} {}\n", self.offset.0, self.offset.1).as_bytes())
				.map(|_| DataStream { inner: s, fail: None }))
	}

	fn is_valid(&self, _conn: &mut Self::Connection) -> Result<(), Self::Error>
	{
		Ok(())
	}
	fn has_broken(&self, conn: &mut Self::Connection) -> bool
	{
		if let Some(err) = &conn.fail { warn!("discarded: {}", err); }
		conn.fail.is_some()
	}
}
