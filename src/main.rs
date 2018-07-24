
#[macro_use]
extern crate structopt;
extern crate rand;
extern crate image;
extern crate rayon;

use std::{
	io::prelude::*,
	net::TcpStream,
	path::PathBuf
};

use structopt::StructOpt;
use rand::{thread_rng, Rng};
use image::{Pixel, GenericImage, DynamicImage};
use rayon::prelude::*;

#[derive(StructOpt, Debug)]
struct Opt 
{
    #[structopt(parse(from_os_str), help = "Image to spray")]
    image: PathBuf,
	
	#[structopt(help = "The host to connect to")]
	host: String,
}


fn main() 
{
	let opt = Opt::from_args();
	let image: DynamicImage = image::open(&opt.image).expect("failed to load image");

	//image = image.resize(256, 256, image::FilterType::Nearest);

	let (w,h) = image.dimensions();
	let (xoff,yoff) = (0,0);
	let n = 8;


	let mut rng = thread_rng();
	let mut l = image.pixels().filter_map(|(x,y,p)|
		{ 
			let (r,g,b,a) = p.channels4();
			//if a > 0 { Some(format!("PX {} {} {:X}{:X}{:X}\n", x+xoff, y+yoff, r, g, b)) } else { None }
			if a > 0 { Some(format!("PX {} {} 0\n", x+xoff, y+yoff)) } else { None }
			//if r > 0 { Some(format!("PX {} {} {:X}\n", x+xoff, y+yoff, r)) } else { None }
			//Some(format!("PX {} {} {:X}\n", x+xoff, y+yoff, r))
		}).collect::<Vec<_>>();

	rng.shuffle(&mut l);

	let buf = l.chunks(l.len()/n+1).map(|c|
		{
			c.into_iter().fold(Vec::<u8>::new(), |mut v, s|
			{
				v.extend_from_slice(s.as_bytes());
				v
			})
		}).collect::<Vec<_>>();
	
	println!("Connecting to {}...", &opt.host);

	buf.par_iter().for_each(|buf|
	{
		let res = TcpStream::connect(&opt.host);
		if res.is_err() { return; }

		let mut stream = res.unwrap();
		stream.set_nodelay(true).ok();
		stream.write_all(format!("OFFSET {} {}", (1920-w)/2, (1080-h)/2).as_bytes()).ok();
		loop {
			stream.write_all(&buf).expect("dead");
		}
	});
}
