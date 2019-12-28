use std::{
	path::PathBuf,
	str::FromStr,
};

use image::{Pixel, GenericImageView};
use rand::{seq::SliceRandom};

use clap::{arg_enum};
use structopt::{StructOpt};

use futures::{
	future::FutureExt,
	stream::StreamExt,
	sink::SinkExt,
};
use tokio::{*,
	io::AsyncWriteExt,
};
use tokio_util::codec::{Decoder};


use log;


arg_enum!{
	#[derive(Debug,Copy,Clone,PartialEq)]
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
	/// The host to connect to
	#[structopt()]
	host: std::net::SocketAddr,

	/// Number of connections
	#[structopt(short = "n", default_value = "8")]
	num: usize,

	/// Image to spray
	#[structopt(parse(from_os_str))]
	image: PathBuf,

	/// Resize image
	#[structopt(short = "r")]
	resize: Option<String>,

	/// Resize image
	#[structopt(short = "o")]
	offset: Option<String>,

	/// Filter to use
	#[structopt(short = "f", default_value="RGBA")]
	filter: Filter,

	/// Use a single color mask
	#[structopt(long = "filter-color", default_value="255")]
	color: u8,

	/// Mirror image
	#[structopt(long)]
	mirror: bool,

	/// Mirror image
	#[structopt(long)]
	mirror_v: bool,


	/// Disable offset option
	#[structopt(long = "no-offset")]
	no_offset: bool,

	/// Do not compact pixels
	#[structopt(short = "l", long)]
	lossless: bool,

	/// Do not compact pixels
	#[structopt(short = "c", long)]
	same_ch_opt: bool
}


fn main() -> Result<(), Box<dyn std::error::Error>>
{
	env_logger::Builder::from_default_env()
		.format_timestamp_millis()
		.filter_level(log::LevelFilter::Debug)
		.init();

	let opt = Opt::from_args();
	log::info!("pixelspray: {:?}", &opt);

	runtime::Builder::new()
		.threaded_scheduler()
		.thread_name("rt-worker")
		.thread_stack_size(80 * 1024) // musl libc stack size
		.enable_all()
		.build()?
		.block_on(run(opt))
}

async fn run(opt: Opt) -> Result<(), Box<dyn std::error::Error>>
{
	let mut image = image::open(&opt.image)?;
	if opt.mirror_v {
		image = image::DynamicImage::ImageRgba8(image::imageops::flip_horizontal(&image));
	}
	if opt.mirror {
		image = image::DynamicImage::ImageRgba8(image::imageops::flip_vertical(&image));
	}

	log::info!("connecting to {}...", opt.host);

	let stream = net::TcpStream::connect(opt.host).await?;
	let codec = tokio_util::codec::LinesCodec::new();
	let mut stream = codec.framed(stream);

	stream.send("SIZE".to_owned()).await?;
	let res = stream.next().await
	                .and_then(|res| res.ok());
	let (sw,sh) = match res {
		Some(s) => {
			log::debug!("SIZE: {}", s);
			let mut i = s.split_ascii_whitespace()
						 .skip(1)
			             .map(|s| u32::from_str(s).unwrap());

			let w = i.next().unwrap();
			let h = i.next().unwrap();
			(w, h)
		},
		None => (1024, 768),
	};
	std::mem::drop(stream);

	let (w,h) = image.dimensions();

	if let Some(resize) = opt.resize.as_ref() {
		let mut i = resize.split('x').map(|s| u32::from_str(s).unwrap());
		let w = i.next().unwrap();
		let h = i.next().unwrap();
		image = image.resize(w, h, image::imageops::FilterType::Lanczos3);
	} else if  w > sw || h > sh {
		image = image.resize(sw, sh, image::imageops::FilterType::Lanczos3);
	}

	let (w,h) = image.dimensions();

	let (xoff,yoff) = if let Some(offset) = opt.offset.as_ref() {
		let mut i = offset.split('x').map(|s| u32::from_str(s).unwrap());
		let x = i.next().unwrap();
		let y = i.next().unwrap();
		(x, y)
	} else {
		(sw-w, sh-h)
	};

	//image = image.resize(256, 256, image::FilterType::Nearest);
	//image = image.grayscale();

	log::info!("screen: {}x{} image: {}x{} offset: {}x{}", sw, sh, w, h, xoff, yoff);

	let mut pxls = image.pixels()
		.filter(|pixel|
		{
			let (_x, _y, color) = pixel;
			let (_r,_g,_b,a) = color.channels4();
			if opt.lossless { a != 0 } else { a > 0xf }
		})
		.map(|(mut x, mut y, color)| {

			let (mut r,g,b,a) = color.to_rgba().channels4();
			let mut ch = color.channels().len();

			if opt.no_offset {
				x += xoff;
				y += yoff;
			}

			if ch > 3 && a == 0xff {
				ch = 3;
			}

			let mut filter = opt.filter;
			if opt.same_ch_opt {
				if filter != Filter::Mask {
					if opt.lossless {
						if r == g && g == b {
							filter = Filter::Grey;
						}
					} else {
						let rg = (r as i32 - g as i32).abs();
						let gb  = (g as i32 - b as i32).abs();
						let br  = (b as i32 - r as i32).abs();
						if *[rg, gb, br].iter().max().unwrap() <= 4 {
							r = ((r as usize + g as usize + b as usize) / 3) as u8;
							filter = Filter::Grey;
						}
					}
				}
			}

			match filter
			{
				Filter::Mask => format!("PX {} {} {:02X}\n", x, y, opt.color),
				Filter::Grey => format!("PX {} {} {:02X}\n", x, y, r),
				Filter::RGBA if ch == 3 => format!("PX {} {} {:02X}{:02X}{:02X}\n", x, y, r, g, b),
				Filter::RGBA => format!("PX {} {} {:02X}{:02X}{:02X}{:02X}\n", x, y, r, g, b, a),
			}
		})
		.collect::<Vec<_>>();

	pxls.shuffle(&mut rand::thread_rng());

	let chunk_len = (pxls.len() + pxls.len() % opt.num) / opt.num;

	println!("Pixels: {}", pxls.len());
	println!("Chunks: {} a {}", pxls.len() / chunk_len, chunk_len);

	let chunks = pxls.chunks(chunk_len)
		.map(|chunk| chunk.concat())
//		.map(Arc::new)
		.collect::<Vec<_>>();

	let mut tasks = chunks.into_iter()
		.map(|chunk| spawn(client(opt.host, chunk, (xoff, yoff))))
		.collect::<futures::stream::FuturesUnordered<_>>();

	loop {
		futures::select! {
			_ = signal::ctrl_c().fuse() => {
				log::info!("stopping...");
				break;
			},
			chunk = tasks.next() => {
				if let Some(chunk) = chunk.and_then(|res| res.ok()) {
					log::info!("respawning...");
					tasks.push(spawn(client(opt.host, chunk, (xoff, yoff))));
				} else {
					break;
				}
			},
		};
	}

	Ok(())
}

async fn client(host_addr: std::net::SocketAddr, chunk: String, offset: (u32, u32)) -> String {
	let mut stream = match net::TcpStream::connect(host_addr).await {
		Ok(v) => v,
		Err(err) => {
			log::error!("failed to connect: {}", err);
			return chunk;
		}
	};
	log::info!("connected...");
	if let Err(err) = stream.set_nodelay(true) {
		log::warn!("failed to set no delay: {}", err);
	}

	let offset = format!("OFFSET {} {}\n", offset.0, offset.1);
	if let Err(err) = stream.write_all(offset.as_bytes()).await {
		log::error!("failed to set offset: {}", err);
		return chunk;
	}

	let buf = chunk.as_bytes();
	loop {
		//log::debug!("sending {} bytes", buf.len());
		if let Err(err) = stream.write_all(buf).await {
			log::error!("failed to send: {}", err);
			break;
		}

	}

	chunk
}
