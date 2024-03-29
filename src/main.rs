use std::{
	path::PathBuf,
	str::FromStr,
	sync::Arc, convert::TryInto,
};

use anyhow::Context;
use chrono::offset;
use image::{Pixel, GenericImageView};
use rand::seq::SliceRandom;

use clap::{Parser, ValueEnum};

use futures::{
	future::FutureExt,
	stream::StreamExt,
	sink::SinkExt,
};
use tokio::{*,
	io::AsyncWriteExt,
};
use tokio_util::codec::Decoder;

use tracing as log;


#[derive(Parser, Debug)]
#[clap(about, version)]
struct Opt
{
	/// The host to connect to
	#[arg()]
	host: std::net::SocketAddr,

	/// Number of connections
	#[arg(short = 'n', default_value_t = 8)]
	num: usize,

	/// Image to spray
	#[arg(value_parser)]
	image: PathBuf,

	/// Resize image
	#[arg(short = 'r')]
	resize: Option<String>,

	/// Resize image
	#[arg(short = 'o')]
	offset: Option<String>,

	/// Filter to use
	#[arg(short = 'f', default_value="rgba")]
	filter: Filter,

	/// Use a single color mask
	#[arg(long = "filter-color", default_value_t=255)]
	color: u8,

	/// Mirror image
	#[arg(long)]
	mirror: bool,

	/// Mirror image
	#[arg(long)]
	mirror_v: bool,


	/// Disable offset option
	#[arg(long = "no-offset")]
	no_offset: bool,

	/// Do not compact pixels
	#[arg(short = 'l', long)]
	lossless: bool,

	/// Do not compact pixels
	#[arg(short = 'c', long)]
	same_ch_opt: bool
}

#[derive(ValueEnum,Debug,Copy,Clone,PartialEq)]
enum Filter
{
	Mask,
	Grey,
	RGBA,
}

fn main() -> Result<(), Box<dyn std::error::Error>>
{
	// Logging system init
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::from_default_env()
				.add_directive(tracing_subscriber::filter::LevelFilter::DEBUG.into())
				.add_directive("pixelspray=debug".parse()?)
				//.add_directive("hyper=trace".parse()?)
		)
		.compact()
		.init();

	let opt = Opt::parse();
	log::info!("pixelspray: {:?}", &opt);

	runtime::Builder::new_multi_thread()
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
	let mut stream = stream.into_inner();
	stream.shutdown().await.ok();
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
		let (xstr,ystr) = offset.split_once('x').unwrap();
		//log::debug!("offsetp: {} x {}", xstr, ystr);
		let x = match xstr {
			"E" => sw-w,
			"M" => (sw-w)/2,
			_ => u32::from_str(xstr).unwrap(),	
		};
		let y = match ystr {
			"E" => sh-h,
			"M" => (sh-h)/2,
			_ => u32::from_str(ystr).unwrap(),
		};
		(x,y)
	} else {
		(0,0)
	};

	//image = image.resize(256, 256, image::FilterType::Nearest);
	//image = image.grayscale();

	log::info!("screen: {}x{} image: {}x{} offset: {}x{}", sw, sh, w, h, xoff, yoff);

	let mut pxls = image.pixels()
		.filter(|pixel|
		{
			let (_x, _y, color) = pixel;
			let [_r,_g,_b,a]: [u8; 4] = color.channels()[..].try_into().unwrap();
			if opt.lossless { a != 0 } else { a > 0xf }
		})
		.map(|(mut x, mut y, color)| {

			let [mut r,g,b,a]: [u8; 4] = color.to_rgba().channels()[..].try_into().unwrap();
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

	let chunk_len = 1420; //(pxls.len() + pxls.len() % opt.num) / opt.num;

	println!("Pixels: {}", pxls.len());
	let chunks = pxls.into_iter()
		.fold(vec![ String::with_capacity(chunk_len) ], |mut buf, px|
		{
			let mut chunk = buf.last_mut().unwrap();
			if chunk.len() + px.len() > chunk_len {
				buf.push(String::with_capacity(chunk_len));
				chunk = buf.last_mut().unwrap();
			}
			chunk.push_str(&px);
			buf
		})
		.into_iter().map(Arc::new)
		.collect::<Vec<_>>();

	let offset = (!opt.no_offset).then_some((xoff, yoff));

	println!("Chunks: {} a {}", chunks.len(), chunk_len);
	println!("Offset: {}", offset.map(|(x,y)| format!("{x}x{y}")).unwrap_or_default());

	let mut tasks = futures::stream::FuturesUnordered::new();
	let mut channels = std::collections::HashMap::new();
	for id in 0..opt.num {
		let (tx, task) = client(id, opt.host, offset);
		channels.insert(id, tx);
		tasks.push(task);
	}

	let state = Arc::new(sync::Mutex::new(channels));
	let channels = state.clone();
	spawn(async move {
		let mut chunk_iter = chunks.into_iter().cycle();
		loop {
			let mut channels = channels.lock().await;
/*			let sends = channels.values_mut()
				.zip(chunk_iter.by_ref())
				.map(|(tx, chunk)| tx.send(chunk.clone()));

			futures::future::select_all(sends).await;
*/
			let mut broken = Vec::new();
			for ((&id, tx), chunk) in channels.iter_mut()
				.zip(chunk_iter.by_ref())
			{
				if let Err(_err) = tx.send(chunk.clone()).await {
					broken.push(id);
				}
			}
			for id in broken {
				channels.remove(&id);
			}
		}
	});

	let channels = state.clone();
	loop {
		futures::select! {
			_ = signal::ctrl_c().fuse() => {
				break;
			},
			id = tasks.next() => {
				log::debug!("meh {:?}", id);
				if let Some(Err(err)) = id {
					continue;
				}
				match id {
					None => break,
					Some(Err(err)) => break,
					Some(Ok(Ok(_))) => break,
					Some(Ok(Err(err))) => break,
					Some(Ok(Ok(id))) => {
						log::info!("{}: respawning...", id);
						let (tx, task) = client(id, opt.host, offset);
						channels.lock().await.entry(id).and_modify(|v| *v = tx);
						tasks.push(task);	
					},
				}
			},
		};
	}
	log::info!("stopping...");
	Ok(())
}

fn client(id: usize, host_addr: std::net::SocketAddr, offset: Option<(u32, u32)>) -> (sync::mpsc::Sender<Arc<String>>, task::JoinHandle<anyhow::Result<usize>>) {
	let (tx, mut rx) = sync::mpsc::channel::<Arc<String>>(4);

	let task = spawn(async move {
		let mut stream = net::TcpStream::connect(host_addr).await
			.context("failed to connect")?;

		log::info!("{}: connected...", id);
		if let Err(err) = stream.set_nodelay(true) {
			log::warn!("{}: failed to set no delay: {}", id, err);
		}

		if let Some(offset) = offset {
			let offset = format!("OFFSET {} {}\n", offset.0, offset.1);
			stream.write_all(offset.as_bytes()).await
				.context("failed to send offset")?;
		}

		while let Some(chunk) = rx.recv().await {
			//log::debug!("sending {} bytes: {}...", chunk.len(), chunk.split_at(16).0);
			stream.write_all(chunk.as_bytes()).await
				.context("failed to send chunk")?;
		}

		Ok(id)
	});

	(tx, task)
}
