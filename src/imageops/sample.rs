//! Functions and filters for the sampling of pixels.

// See http://cs.brown.edu/courses/cs123/lectures/08_Image_Processing_IV.pdf
// for some of the theory behind image scaling and convolution

use std::f32;

use num_traits::Zero;
use pixeli::{
    ContiguousPixel, Enlargeable, FromComponentCommon, FromPixelCommon, Pixel, PixelComponent, Rgba,
};

use crate::color::Blend;
use crate::image::{GenericImage, GenericImageView};
use crate::utils::clamp;
use crate::{ImageBuffer, Rgba32FImage};

/// Available Sampling Filters.
///
/// ## Examples
///
/// To test the different sampling filters on a real example, you can find two
/// examples called
/// [`scaledown`](https://github.com/image-rs/image/tree/master/examples/scaledown)
/// and
/// [`scaleup`](https://github.com/image-rs/image/tree/master/examples/scaleup)
/// in the `examples` directory of the crate source code.
///
/// Here is a 3.58 MiB
/// [test image](https://github.com/image-rs/image/blob/master/examples/scaledown/test.jpg)
/// that has been scaled down to 300x225 px:
///
/// <!-- NOTE: To test new test images locally, replace the GitHub path with `../../../docs/` -->
/// <div style="display: flex; flex-wrap: wrap; align-items: flex-start;">
///   <div style="margin: 0 8px 8px 0;">
///     <img src="https://raw.githubusercontent.com/image-rs/image/master/examples/scaledown/scaledown-test-near.png" title="Nearest"><br>
///     Nearest Neighbor
///   </div>
///   <div style="margin: 0 8px 8px 0;">
///     <img src="https://raw.githubusercontent.com/image-rs/image/master/examples/scaledown/scaledown-test-tri.png" title="Triangle"><br>
///     Linear: Triangle
///   </div>
///   <div style="margin: 0 8px 8px 0;">
///     <img src="https://raw.githubusercontent.com/image-rs/image/master/examples/scaledown/scaledown-test-cmr.png" title="CatmullRom"><br>
///     Cubic: Catmull-Rom
///   </div>
///   <div style="margin: 0 8px 8px 0;">
///     <img src="https://raw.githubusercontent.com/image-rs/image/master/examples/scaledown/scaledown-test-gauss.png" title="Gaussian"><br>
///     Gaussian
///   </div>
///   <div style="margin: 0 8px 8px 0;">
///     <img src="https://raw.githubusercontent.com/image-rs/image/master/examples/scaledown/scaledown-test-lcz2.png" title="Lanczos3"><br>
///     Lanczos with window 3
///   </div>
/// </div>
///
/// ## Speed
///
/// Time required to create each of the examples above, tested on an Intel
/// i7-4770 CPU with Rust 1.37 in release mode:
///
/// <table style="width: auto;">
///   <tr>
///     <th>Nearest</th>
///     <td>31 ms</td>
///   </tr>
///   <tr>
///     <th>Triangle</th>
///     <td>414 ms</td>
///   </tr>
///   <tr>
///     <th>CatmullRom</th>
///     <td>817 ms</td>
///   </tr>
///   <tr>
///     <th>Gaussian</th>
///     <td>1180 ms</td>
///   </tr>
///   <tr>
///     <th>Lanczos3</th>
///     <td>1170 ms</td>
///   </tr>
/// </table>
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilterType {
    /// Nearest Neighbor
    Nearest,

    /// Linear Filter
    Triangle,

    /// Cubic Filter
    CatmullRom,

    /// Gaussian Filter
    Gaussian,

    /// Lanczos with window 3
    Lanczos3,
}

/// A Representation of a separable filter.
pub(crate) struct Filter<'a> {
    /// The filter's filter function.
    pub(crate) kernel: Box<dyn Fn(f32) -> f32 + 'a>,

    /// The window on which this filter operates.
    pub(crate) support: f32,
}

// sinc function: the ideal sampling filter.
fn sinc(t: f32) -> f32 {
    let a = t * f32::consts::PI;

    if t == 0.0 {
        1.0
    } else {
        a.sin() / a
    }
}

// lanczos kernel function. A windowed sinc function.
fn lanczos(x: f32, t: f32) -> f32 {
    if x.abs() < t {
        sinc(x) * sinc(x / t)
    } else {
        0.0
    }
}

// Calculate a splice based on the b and c parameters.
// from authors Mitchell and Netravali.
fn bc_cubic_spline(x: f32, b: f32, c: f32) -> f32 {
    let a = x.abs();

    let k = if a < 1.0 {
        (12.0 - 9.0 * b - 6.0 * c) * a.powi(3)
            + (-18.0 + 12.0 * b + 6.0 * c) * a.powi(2)
            + (6.0 - 2.0 * b)
    } else if a < 2.0 {
        (-b - 6.0 * c) * a.powi(3)
            + (6.0 * b + 30.0 * c) * a.powi(2)
            + (-12.0 * b - 48.0 * c) * a
            + (8.0 * b + 24.0 * c)
    } else {
        0.0
    };

    k / 6.0
}

/// The Gaussian Function.
/// ```r``` is the standard deviation.
pub(crate) fn gaussian(x: f32, r: f32) -> f32 {
    ((2.0 * f32::consts::PI).sqrt() * r).recip() * (-x.powi(2) / (2.0 * r.powi(2))).exp()
}

/// Calculate the lanczos kernel with a window of 3
pub(crate) fn lanczos3_kernel(x: f32) -> f32 {
    lanczos(x, 3.0)
}

/// Calculate the gaussian function with a
/// standard deviation of 0.5
pub(crate) fn gaussian_kernel(x: f32) -> f32 {
    gaussian(x, 0.5)
}

/// Calculate the Catmull-Rom cubic spline.
/// Also known as a form of `BiCubic` sampling in two dimensions.
pub(crate) fn catmullrom_kernel(x: f32) -> f32 {
    bc_cubic_spline(x, 0.0, 0.5)
}

/// Calculate the triangle function.
/// Also known as `BiLinear` sampling in two dimensions.
pub(crate) fn triangle_kernel(x: f32) -> f32 {
    if x.abs() < 1.0 {
        1.0 - x.abs()
    } else {
        0.0
    }
}

/// Calculate the box kernel.
/// Only pixels inside the box should be considered, and those
/// contribute equally.  So this method simply returns 1.
pub(crate) fn box_kernel(_x: f32) -> f32 {
    1.0
}

// Sample the rows of the supplied image using the provided filter.
// The height of the image remains unchanged.
// ```new_width``` is the desired width of the new image
// ```filter``` is the filter to use for sampling.
// ```image``` is not necessarily Rgba and the order of channels is passed through.
fn horizontal_sample<P>(
    image: &Rgba32FImage,
    new_width: u32,
    filter: &mut Filter,
) -> ImageBuffer<P, Vec<P::Component>>
where
    P: ContiguousPixel + FromPixelCommon<Rgba<f32>>,
{
    let (width, height) = image.dimensions();
    let mut out = ImageBuffer::new(new_width, height);
    let mut ws = Vec::new();

    let ratio = width as f32 / new_width as f32;
    let sratio = if ratio < 1.0 { 1.0 } else { ratio };
    let src_support = filter.support * sratio;

    for outx in 0..new_width {
        // Find the point in the input image corresponding to the centre
        // of the current pixel in the output image.
        let inputx = (outx as f32 + 0.5) * ratio;

        // Left and right are slice bounds for the input pixels relevant
        // to the output pixel we are calculating.  Pixel x is relevant
        // if and only if (x >= left) && (x < right).

        // Invariant: 0 <= left < right <= width

        let left = (inputx - src_support).floor() as i64;
        let left = clamp(left, 0, <i64 as From<_>>::from(width) - 1) as u32;

        let right = (inputx + src_support).ceil() as i64;
        let right = clamp(
            right,
            <i64 as From<_>>::from(left) + 1,
            <i64 as From<_>>::from(width),
        ) as u32;

        // Go back to left boundary of pixel, to properly compare with i
        // below, as the kernel treats the centre of a pixel as 0.
        let inputx = inputx - 0.5;

        ws.clear();
        let mut sum = 0.0;
        for i in left..right {
            let w = (filter.kernel)((i as f32 - inputx) / sratio);
            ws.push(w);
            sum += w;
        }
        ws.iter_mut().for_each(|w| *w /= sum);

        for y in 0..height {
            let mut t = Rgba::<f32> {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };

            for (i, w) in ws.iter().enumerate() {
                let p = image.get_pixel(left + i as u32, y);

                t.r += p.r * w;
                t.g += p.g * w;
                t.b += p.b * w;
                t.a += p.a * w;
            }

            let t = P::from_pixel_common(t);

            out.put_pixel(outx, y, t);
        }
    }

    out
}

/// Linearly sample from an image using coordinates in [0, 1].
pub fn sample_bilinear<P>(img: &impl GenericImageView<Pixel = P>, u: f32, v: f32) -> Option<P>
where
    P: ContiguousPixel,
    f32: FromComponentCommon<P::Component>,
    P::Component: FromComponentCommon<f32>,
{
    if ![u, v].iter().all(|c| (0.0..=1.0).contains(c)) {
        return None;
    }

    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    let ui = w as f32 * u - 0.5;
    let vi = h as f32 * v - 0.5;
    interpolate_bilinear(
        img,
        ui.max(0.).min((w - 1) as f32),
        vi.max(0.).min((h - 1) as f32),
    )
}

/// Sample from an image using coordinates in [0, 1], taking the nearest coordinate.
pub fn sample_nearest<P: Pixel>(
    img: &impl GenericImageView<Pixel = P>,
    u: f32,
    v: f32,
) -> Option<P> {
    if ![u, v].iter().all(|c| (0.0..=1.0).contains(c)) {
        return None;
    }

    let (w, h) = img.dimensions();
    let ui = w as f32 * u - 0.5;
    let ui = ui.max(0.).min((w.saturating_sub(1)) as f32);

    let vi = h as f32 * v - 0.5;
    let vi = vi.max(0.).min((h.saturating_sub(1)) as f32);
    interpolate_nearest(img, ui, vi)
}

/// Sample from an image using coordinates in [0, w-1] and [0, h-1], taking the
/// nearest pixel.
///
/// Coordinates outside the image bounds will return `None`, however the
/// behavior for points within half a pixel of the image bounds may change in
/// the future.
pub fn interpolate_nearest<P: Pixel>(
    img: &impl GenericImageView<Pixel = P>,
    x: f32,
    y: f32,
) -> Option<P> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    if !(0.0..=((w - 1) as f32)).contains(&x) {
        return None;
    }
    if !(0.0..=((h - 1) as f32)).contains(&y) {
        return None;
    }

    Some(img.get_pixel(x.round() as u32, y.round() as u32))
}

/// Linearly sample from an image using coordinates in [0, w-1] and [0, h-1].
pub fn interpolate_bilinear<P>(img: &impl GenericImageView<Pixel = P>, x: f32, y: f32) -> Option<P>
where
    P: Pixel,
    f32: FromComponentCommon<P::Component>,
    P::Component: FromComponentCommon<f32>,
{
    // assumption needed for correctness of pixel creation
    assert!(P::COMPONENT_COUNT <= 4);

    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    if !(0.0..=((w - 1) as f32)).contains(&x) {
        return None;
    }
    if !(0.0..=((h - 1) as f32)).contains(&y) {
        return None;
    }

    // keep these as integers, for fewer FLOPs
    let uf = x.floor() as u32;
    let vf = y.floor() as u32;
    let uc = (uf + 1).min(w - 1);
    let vc = (vf + 1).min(h - 1);

    // clamp coords to the range of the image
    let mut sxx = [[0.; 4]; 4];

    // do not use Array::map, as it can be slow with high stack usage,
    // for [[f32; 4]; 4].

    // convert samples to f32
    // currently rgba is the largest one,
    // so just store as many items as necessary,
    // because there's not a simple way to be generic over all of them.
    let mut compute = |u: u32, v: u32, i| {
        let p = img.get_pixel(u, v);
        for (j, c) in p.component_array().into_iter().enumerate() {
            sxx[j][i] = f32::from_component_common(c);
        }
        p
    };

    // hacky reuse since cannot construct a generic Pixel
    let out: P = compute(uf, vf, 0);
    compute(uf, vc, 1);
    compute(uc, vf, 2);
    compute(uc, vc, 3);

    // weights, the later two are independent from the first 2 for better vectorization.
    let ufw = x - uf as f32;
    let vfw = y - vf as f32;
    let ucw = (uf + 1) as f32 - x;
    let vcw = (vf + 1) as f32 - y;

    // https://en.wikipedia.org/wiki/Bilinear_interpolation#Weighted_mean
    // the distance between pixels is 1 so there is no denominator
    let wff = ucw * vcw;
    let wfc = ucw * vfw;
    let wcf = ufw * vcw;
    let wcc = ufw * vfw;
    // was originally assert, but is actually not a cheap computation
    debug_assert!(f32::abs((wff + wfc + wcf + wcc) - 1.) < 1e-3);

    // hack to see if primitive is an integer or a float
    let is_float = f32::from_component_common(P::Component::COMPONENT_MAX) == 1.0;

    let out = P::from_components(out.component_array().into_iter().enumerate().map(|(i, _)| {
        let v = wff * sxx[i][0] + wfc * sxx[i][1] + wcf * sxx[i][2] + wcc * sxx[i][3];
        // this rounding may introduce quantization errors,
        // Specifically what is meant is that many samples may deviate
        // from the mean value of the originals, but it's not possible to fix that.
        <P::Component as FromComponentCommon<f32>>::from_component_common(if is_float {
            v
        } else {
            v.round()
        })
    }));

    Some(out)
}

// Sample the columns of the supplied image using the provided filter.
// The width of the image remains unchanged.
// ```new_height``` is the desired height of the new image
// ```filter``` is the filter to use for sampling.
// The return value is not necessarily Rgba, the underlying order of channels in ```image``` is
// preserved.
fn vertical_sample<I, P>(image: &I, new_height: u32, filter: &mut Filter) -> Rgba32FImage
where
    I: GenericImageView<Pixel = P>,
    P: ContiguousPixel,
    Rgba<f32>: FromPixelCommon<P>,
{
    let (width, height) = image.dimensions();
    let mut out = ImageBuffer::new(width, new_height);
    let mut ws = Vec::new();

    let ratio = height as f32 / new_height as f32;
    let sratio = if ratio < 1.0 { 1.0 } else { ratio };
    let src_support = filter.support * sratio;

    for outy in 0..new_height {
        // For an explanation of this algorithm, see the comments
        // in horizontal_sample.
        let inputy = (outy as f32 + 0.5) * ratio;

        let left = (inputy - src_support).floor() as i64;
        let left = clamp(left, 0, <i64 as From<_>>::from(height) - 1) as u32;

        let right = (inputy + src_support).ceil() as i64;
        let right = clamp(
            right,
            <i64 as From<_>>::from(left) + 1,
            <i64 as From<_>>::from(height),
        ) as u32;

        let inputy = inputy - 0.5;

        ws.clear();
        let mut sum = 0.0;
        for i in left..right {
            let w = (filter.kernel)((i as f32 - inputy) / sratio);
            ws.push(w);
            sum += w;
        }
        ws.iter_mut().for_each(|w| *w /= sum);

        for x in 0..width {
            let mut t = Rgba::<f32> {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };

            for (i, w) in ws.iter().enumerate() {
                let p = image.get_pixel(x, left + i as u32);
                let p = Rgba::<f32>::from_pixel_common(p);

                t.r += p.r * w;
                t.g += p.g * w;
                t.b += p.b * w;
                t.a += p.a * w;
            }

            out.put_pixel(x, outy, t);
        }
    }

    out
}

/// Local struct for keeping track of pixel sums for fast thumbnail averaging
struct ThumbnailSum<S: Enlargeable>(S::Larger, S::Larger, S::Larger, S::Larger);

impl<S> ThumbnailSum<S>
where
    S: Enlargeable,
    S::Larger: FromComponentCommon<S>,
{
    fn zeroed() -> Self {
        ThumbnailSum(
            S::Larger::zero(),
            S::Larger::zero(),
            S::Larger::zero(),
            S::Larger::zero(),
        )
    }

    fn sample_val(val: S) -> S::Larger {
        S::Larger::from_component_common(val)
    }

    fn add_pixel<P>(&mut self, pixel: P)
    where
        P: Pixel<Component = S>,
    {
        let mut iter = pixel.component_array().into_iter();
        self.0 += Self::sample_val(iter.next().unwrap());
        self.1 += Self::sample_val(iter.next().unwrap());
        self.2 += Self::sample_val(iter.next().unwrap());
        self.3 += Self::sample_val(iter.next().unwrap());
    }
}

/// Resize the supplied image to the specific dimensions.
///
/// For downscaling, this method uses a fast integer algorithm where each source pixel contributes
/// to exactly one target pixel.  May give aliasing artifacts if new size is close to old size.
///
/// In case the current width is smaller than the new width or similar for the height, another
/// strategy is used instead.  For each pixel in the output, a rectangular region of the input is
/// determined, just as previously.  But when no input pixel is part of this region, the nearest
/// pixels are interpolated instead.
///
/// For speed reasons, all interpolation is performed linearly over the colour values.  It will not
/// take the pixel colour spaces into account.
pub fn thumbnail<I, P, S>(image: &I, new_width: u32, new_height: u32) -> ImageBuffer<P, Vec<S>>
where
    I: GenericImageView<Pixel = P>,
    P: Pixel<Component = S> + ContiguousPixel + 'static,
    S: PixelComponent + Enlargeable + FromComponentCommon<f32> + 'static,
    f32: FromComponentCommon<S::Larger>,
    S::Larger: FromComponentCommon<S>,
    S::Larger: FromComponentCommon<u32>,
    S: FromComponentCommon<f32>,
    f32: FromComponentCommon<S>,
{
    let (width, height) = image.dimensions();
    let mut out = ImageBuffer::new(new_width, new_height);
    if height == 0 || width == 0 {
        return out;
    }

    let x_ratio = width as f32 / new_width as f32;
    let y_ratio = height as f32 / new_height as f32;

    for outy in 0..new_height {
        let bottomf = outy as f32 * y_ratio;
        let topf = bottomf + y_ratio;

        let bottom = clamp(bottomf.ceil() as u32, 0, height - 1);
        let top = clamp(topf.ceil() as u32, bottom, height);

        for outx in 0..new_width {
            let leftf = outx as f32 * x_ratio;
            let rightf = leftf + x_ratio;

            let left = clamp(leftf.ceil() as u32, 0, width - 1);
            let right = clamp(rightf.ceil() as u32, left, width);

            let pixel = if bottom != top && left != right {
                thumbnail_sample_block(image, left, right, bottom, top)
            } else if bottom != top {
                // && left == right
                // In the first column we have left == 0 and right > ceil(y_scale) > 0 so this
                // assertion can never trigger.
                debug_assert!(
                    left > 0 && right > 0,
                    "First output column must have corresponding pixels"
                );

                let fraction_horizontal = (leftf.fract() + rightf.fract()) / 2.;
                thumbnail_sample_fraction_horizontal(
                    image,
                    right - 1,
                    fraction_horizontal,
                    bottom,
                    top,
                )
            } else if left != right {
                // && bottom == top
                // In the first line we have bottom == 0 and top > ceil(x_scale) > 0 so this
                // assertion can never trigger.
                debug_assert!(
                    bottom > 0 && top > 0,
                    "First output row must have corresponding pixels"
                );

                let fraction_vertical = (topf.fract() + bottomf.fract()) / 2.;
                thumbnail_sample_fraction_vertical(image, left, right, top - 1, fraction_vertical)
            } else {
                // bottom == top && left == right
                let fraction_horizontal = (topf.fract() + bottomf.fract()) / 2.;
                let fraction_vertical = (leftf.fract() + rightf.fract()) / 2.;

                thumbnail_sample_fraction_both::<_, _, S>(
                    image,
                    right - 1,
                    fraction_horizontal,
                    top - 1,
                    fraction_vertical,
                )
            };

            out.put_pixel(outx, outy, pixel);
        }
    }

    out
}

/// Get a pixel for a thumbnail where the input window encloses at least a full pixel.
fn thumbnail_sample_block<I, P, S>(image: &I, left: u32, right: u32, bottom: u32, top: u32) -> P
where
    I: GenericImageView<Pixel = P>,
    P: Pixel<Component = S>,
    S: PixelComponent + Enlargeable,
    S::Larger: FromComponentCommon<S>,
    S::Larger: FromComponentCommon<u32>,
{
    let mut sum = ThumbnailSum::zeroed();

    for y in bottom..top {
        for x in left..right {
            let k = image.get_pixel(x, y);
            sum.add_pixel(k);
        }
    }

    let n = S::Larger::from_component_common((right - left) * (top - bottom));
    let round = n / S::Larger::from_component_common(2);
    P::from_components([
        S::clamp_from((sum.0 + round) / n),
        S::clamp_from((sum.1 + round) / n),
        S::clamp_from((sum.2 + round) / n),
        S::clamp_from((sum.3 + round) / n),
    ])
}

/// Get a thumbnail pixel where the input window encloses at least a vertical pixel.
fn thumbnail_sample_fraction_horizontal<I, P, S>(
    image: &I,
    left: u32,
    fraction_horizontal: f32,
    bottom: u32,
    top: u32,
) -> P
where
    I: GenericImageView<Pixel = P>,
    P: Pixel<Component = S>,
    S: PixelComponent + Enlargeable,
    S: FromComponentCommon<f32>,
    f32: FromComponentCommon<S::Larger>,
    S::Larger: FromComponentCommon<S>,
{
    let fract = fraction_horizontal;

    let mut sum_left = ThumbnailSum::zeroed();
    let mut sum_right = ThumbnailSum::zeroed();
    for x in bottom..top {
        let k_left = image.get_pixel(left, x);
        sum_left.add_pixel(k_left);

        let k_right = image.get_pixel(left + 1, x);
        sum_right.add_pixel(k_right);
    }

    // Now we approximate: left/n*(1-fract) + right/n*fract
    let fact_right = fract / ((top - bottom) as f32);
    let fact_left = (1. - fract) / ((top - bottom) as f32);

    let mix_left_and_right = |leftv: S::Larger, rightv: S::Larger| {
        S::from_component_common(
            fact_left * f32::from_component_common(leftv)
                + fact_right * f32::from_component_common(rightv),
        )
    };

    P::from_components([
        mix_left_and_right(sum_left.0, sum_right.0),
        mix_left_and_right(sum_left.1, sum_right.1),
        mix_left_and_right(sum_left.2, sum_right.2),
        mix_left_and_right(sum_left.3, sum_right.3),
    ])
}

/// Get a thumbnail pixel where the input window encloses at least a horizontal pixel.
fn thumbnail_sample_fraction_vertical<I, P, S>(
    image: &I,
    left: u32,
    right: u32,
    bottom: u32,
    fraction_vertical: f32,
) -> P
where
    I: GenericImageView<Pixel = P>,
    P: Pixel<Component = S>,
    S: PixelComponent + Enlargeable,
    S: FromComponentCommon<f32>,
    f32: FromComponentCommon<S::Larger>,
    S::Larger: FromComponentCommon<S>,
{
    let fract = fraction_vertical;

    let mut sum_bot = ThumbnailSum::zeroed();
    let mut sum_top = ThumbnailSum::zeroed();
    for x in left..right {
        let k_bot = image.get_pixel(x, bottom);
        sum_bot.add_pixel(k_bot);

        let k_top = image.get_pixel(x, bottom + 1);
        sum_top.add_pixel(k_top);
    }

    // Now we approximate: bot/n*fract + top/n*(1-fract)
    let fact_top = fract / ((right - left) as f32);
    let fact_bot = (1. - fract) / ((right - left) as f32);

    let mix_bot_and_top = |botv: S::Larger, topv: S::Larger| {
        S::from_component_common(
            fact_bot * f32::from_component_common(botv)
                + fact_top * f32::from_component_common(topv),
        )
    };

    P::from_components([
        mix_bot_and_top(sum_bot.0, sum_top.0),
        mix_bot_and_top(sum_bot.1, sum_top.1),
        mix_bot_and_top(sum_bot.2, sum_top.2),
        mix_bot_and_top(sum_bot.3, sum_top.3),
    ])
}

/// Get a single pixel for a thumbnail where the input window does not enclose any full pixel.
fn thumbnail_sample_fraction_both<I, P, S>(
    image: &I,
    left: u32,
    fraction_vertical: f32,
    bottom: u32,
    fraction_horizontal: f32,
) -> P
where
    I: GenericImageView<Pixel = P>,
    P: Pixel<Component = S>,
    S: PixelComponent + Enlargeable,
    S: FromComponentCommon<f32>,
    f32: FromComponentCommon<S>,
{
    let k_bl = image.get_pixel(left, bottom);
    let k_tl = image.get_pixel(left, bottom + 1);
    let k_br = image.get_pixel(left + 1, bottom);
    let k_tr = image.get_pixel(left + 1, bottom + 1);

    let frac_v = fraction_vertical;
    let frac_h = fraction_horizontal;

    let fact_tr = frac_v * frac_h;
    let fact_tl = frac_v * (1. - frac_h);
    let fact_br = (1. - frac_v) * frac_h;
    let fact_bl = (1. - frac_v) * (1. - frac_h);

    let to_f32 = |x| f32::from_component_common(x);
    let mix = |br: S, tr: S, bl: S, tl: S| {
        S::from_component_common(
            fact_br * to_f32(br)
                + fact_tr * to_f32(tr)
                + fact_bl * to_f32(bl)
                + fact_tl * to_f32(tl),
        )
    };

    P::from_components(
        k_br.component_array()
            .into_iter()
            .zip(k_tr.component_array())
            .zip(k_bl.component_array())
            .zip(k_tl.component_array())
            .map(|(((k_br, k_tr), k_bl), k_tl)| mix(k_br, k_tr, k_bl, k_tl)),
    )
}

/// Perform a 3x3 box filter on the supplied image.
/// ```kernel``` is an array of the filter weights of length 9.
pub fn filter3x3<I, P, S>(image: &I, kernel: &[f32]) -> ImageBuffer<P, Vec<S>>
where
    I: GenericImageView<Pixel = P>,
    P: Pixel<Component = S> + ContiguousPixel + 'static,
    S: PixelComponent + 'static,
    f32: FromComponentCommon<S>,
    P::Component: FromComponentCommon<f32>,
{
    // The kernel's input positions relative to the current pixel.
    let taps: &[(isize, isize)] = &[
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (0, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    let (width, height) = image.dimensions();

    let mut out = ImageBuffer::new(width, height);

    let sum = match kernel.iter().fold(0.0, |s, &item| s + item) {
        x if x == 0.0 => 1.0,
        sum => sum,
    };
    let sum = [sum, sum, sum, sum];

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let mut t = vec![0.0; usize::from(P::COMPONENT_COUNT)];

            // TODO: There is no need to recalculate the kernel for each pixel.
            // Only a subtract and addition is needed for pixels after the first
            // in each row.
            for (&k, &(a, b)) in kernel.iter().zip(taps.iter()) {
                let x0 = x as isize + a;
                let y0 = y as isize + b;

                let p = image.get_pixel(x0 as u32, y0 as u32);

                for (t, c) in t.iter_mut().zip(p.component_array()) {
                    *t += k * f32::from_component_common(c)
                }
            }

            let outpixel = P::from_components(
                t.into_iter()
                    .zip(sum)
                    .map(|(t, sum)| P::Component::from_component_common(t / sum)),
            );

            out.put_pixel(x, y, outpixel);
        }
    }

    out
}

/// Resize the supplied image to the specified dimensions.
/// ```nwidth``` and ```nheight``` are the new dimensions.
/// ```filter``` is the sampling filter to use.
pub fn resize<I: GenericImageView>(
    image: &I,
    nwidth: u32,
    nheight: u32,
    filter: FilterType,
) -> ImageBuffer<I::Pixel, Vec<<I::Pixel as Pixel>::Component>>
where
    I::Pixel: 'static,
    <I::Pixel as Pixel>::Component: 'static,
    Rgba<f32>: FromPixelCommon<I::Pixel>,
    I::Pixel: FromPixelCommon<Rgba<f32>> + Blend,
{
    // check if the new dimensions are the same as the old. if they are, make a copy instead of resampling
    if (nwidth, nheight) == image.dimensions() {
        let mut tmp = ImageBuffer::new(image.width(), image.height());
        tmp.copy_from(image, 0, 0).unwrap();
        return tmp;
    }

    let mut method = match filter {
        FilterType::Nearest => Filter {
            kernel: Box::new(box_kernel),
            support: 0.0,
        },
        FilterType::Triangle => Filter {
            kernel: Box::new(triangle_kernel),
            support: 1.0,
        },
        FilterType::CatmullRom => Filter {
            kernel: Box::new(catmullrom_kernel),
            support: 2.0,
        },
        FilterType::Gaussian => Filter {
            kernel: Box::new(gaussian_kernel),
            support: 3.0,
        },
        FilterType::Lanczos3 => Filter {
            kernel: Box::new(lanczos3_kernel),
            support: 3.0,
        },
    };

    // Note: tmp is not necessarily actually Rgba
    let tmp: Rgba32FImage = vertical_sample(image, nheight, &mut method);
    horizontal_sample(&tmp, nwidth, &mut method)
}

/// Performs a Gaussian blur on the supplied image.
/// ```sigma``` is a measure of how much to blur by.
pub fn blur<I: GenericImageView>(
    image: &I,
    sigma: f32,
) -> ImageBuffer<I::Pixel, Vec<<I::Pixel as Pixel>::Component>>
where
    I::Pixel: 'static,
    Rgba<f32>: FromPixelCommon<I::Pixel>,
    I::Pixel: FromPixelCommon<Rgba<f32>>,
{
    let sigma = if sigma <= 0.0 { 1.0 } else { sigma };

    let mut method = Filter {
        kernel: Box::new(|x| gaussian(x, sigma)),
        support: 2.0 * sigma,
    };

    let (width, height) = image.dimensions();

    // Keep width and height the same for horizontal and
    // vertical sampling.
    // Note: tmp is not necessarily actually Rgba
    let tmp: Rgba32FImage = vertical_sample(image, height, &mut method);
    horizontal_sample(&tmp, width, &mut method)
}

/// Performs an unsharpen mask on the supplied image.
/// ```sigma``` is the amount to blur the image by.
/// ```threshold``` is the threshold for minimal brightness change that will be sharpened.
///
/// See <https://en.wikipedia.org/wiki/Unsharp_masking#Digital_unsharp_masking>
pub fn unsharpen<I, P, S>(image: &I, sigma: f32, threshold: i32) -> ImageBuffer<P, Vec<S>>
where
    I: GenericImageView<Pixel = P>,
    P: Pixel<Component = S> + ContiguousPixel + 'static,
    S: PixelComponent + 'static,
    Rgba<f32>: FromPixelCommon<I::Pixel>,
    I::Pixel: FromPixelCommon<Rgba<f32>>,
    i32: FromComponentCommon<S>,
    P::Component: FromComponentCommon<i32>,
{
    let mut tmp = blur(image, sigma);

    let max = S::COMPONENT_MAX;
    let max: i32 = i32::from_component_common(max);
    let (width, height) = image.dimensions();

    for y in 0..height {
        for x in 0..width {
            let a = image.get_pixel(x, y);
            let b = tmp.get_pixel_mut(x, y);

            let new_components = a
                .component_array()
                .into_iter()
                .zip(b.component_array())
                .map(|(a, b)| {
                    let ia: i32 = i32::from_component_common(a);
                    let ib: i32 = i32::from_component_common(b);

                    let diff = ia - ib;

                    if diff.abs() > threshold {
                        let e = clamp(ia + diff, 0, max); // FIXME what does this do for f32? clamp 0-1 integers??

                        P::Component::from_component_common(e)
                    } else {
                        a
                    }
                });

            *b = P::from_components(new_components);
        }
    }

    tmp
}

#[cfg(test)]
mod tests {
    use super::{resize, sample_bilinear, sample_nearest, FilterType};
    use crate::{GenericImageView, ImageBuffer, RgbImage};
    use pixeli::{Pixel, Rgba};
    #[cfg(feature = "benchmarks")]
    use test;

    #[bench]
    #[cfg(all(feature = "benchmarks", feature = "png"))]
    fn bench_resize(b: &mut test::Bencher) {
        use std::path::Path;
        let img = crate::open(Path::new("./examples/fractal.png")).unwrap();
        b.iter(|| {
            test::black_box(resize(&img, 200, 200, FilterType::Nearest));
        });
        b.bytes = 800 * 800 * 3 + 200 * 200 * 3;
    }

    #[test]
    #[cfg(feature = "png")]
    fn test_resize_same_size() {
        use std::path::Path;
        let img = crate::open(Path::new("./examples/fractal.png")).unwrap();
        let resize = img.resize(img.width(), img.height(), FilterType::Triangle);
        assert!(img.pixels().eq(resize.pixels()))
    }

    #[test]
    #[cfg(feature = "png")]
    fn test_sample_bilinear() {
        use std::path::Path;
        let img = crate::open(Path::new("./examples/fractal.png")).unwrap();
        assert!(sample_bilinear(&img, 0., 0.).is_some());
        assert!(sample_bilinear(&img, 1., 0.).is_some());
        assert!(sample_bilinear(&img, 0., 1.).is_some());
        assert!(sample_bilinear(&img, 1., 1.).is_some());
        assert!(sample_bilinear(&img, 0.5, 0.5).is_some());

        assert!(sample_bilinear(&img, 1.2, 0.5).is_none());
        assert!(sample_bilinear(&img, 0.5, 1.2).is_none());
        assert!(sample_bilinear(&img, 1.2, 1.2).is_none());

        assert!(sample_bilinear(&img, -0.1, 0.2).is_none());
        assert!(sample_bilinear(&img, 0.2, -0.1).is_none());
        assert!(sample_bilinear(&img, -0.1, -0.1).is_none());
    }
    #[test]
    #[cfg(feature = "png")]
    fn test_sample_nearest() {
        use std::path::Path;
        let img = crate::open(Path::new("./examples/fractal.png")).unwrap();
        assert!(sample_nearest(&img, 0., 0.).is_some());
        assert!(sample_nearest(&img, 1., 0.).is_some());
        assert!(sample_nearest(&img, 0., 1.).is_some());
        assert!(sample_nearest(&img, 1., 1.).is_some());
        assert!(sample_nearest(&img, 0.5, 0.5).is_some());

        assert!(sample_nearest(&img, 1.2, 0.5).is_none());
        assert!(sample_nearest(&img, 0.5, 1.2).is_none());
        assert!(sample_nearest(&img, 1.2, 1.2).is_none());

        assert!(sample_nearest(&img, -0.1, 0.2).is_none());
        assert!(sample_nearest(&img, 0.2, -0.1).is_none());
        assert!(sample_nearest(&img, -0.1, -0.1).is_none());
    }
    #[test]
    fn test_sample_bilinear_correctness() {
        let img = ImageBuffer::from_fn(2, 2, |x, y| match (x, y) {
            (0, 0) => Rgba {
                r: 255_u8,
                g: 0,
                b: 0,
                a: 0,
            },
            (0, 1) => Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 0,
            },
            (1, 0) => Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 0,
            },
            (1, 1) => Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            _ => panic!(),
        });
        assert_eq!(
            sample_bilinear(&img, 0.5, 0.5),
            Some(Rgba::from_components([64; 4]))
        );
        assert_eq!(
            sample_bilinear(&img, 0.0, 0.0),
            Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 0
            })
        );
        assert_eq!(
            sample_bilinear(&img, 0.0, 1.0),
            Some(Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 0
            })
        );
        assert_eq!(
            sample_bilinear(&img, 1.0, 0.0),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 0
            })
        );
        assert_eq!(
            sample_bilinear(&img, 1.0, 1.0),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255
            })
        );

        assert_eq!(
            sample_bilinear(&img, 0.5, 0.0),
            Some(Rgba {
                r: 128,
                g: 0,
                b: 128,
                a: 0
            })
        );
        assert_eq!(
            sample_bilinear(&img, 0.0, 0.5),
            Some(Rgba {
                r: 128,
                g: 128,
                b: 0,
                a: 0
            })
        );
        assert_eq!(
            sample_bilinear(&img, 0.5, 1.0),
            Some(Rgba {
                r: 0,
                g: 128,
                b: 0,
                a: 128
            })
        );
        assert_eq!(
            sample_bilinear(&img, 1.0, 0.5),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 128,
                a: 128
            })
        );
    }
    #[bench]
    #[cfg(feature = "benchmarks")]
    fn bench_sample_bilinear(b: &mut test::Bencher) {
        use pixeli::Rgba;

        let img = ImageBuffer::from_fn(2, 2, |x, y| match (x, y) {
            (0, 0) => Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 0,
            },
            (0, 1) => Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 0,
            },
            (1, 0) => Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 0,
            },
            (1, 1) => Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            _ => panic!(),
        });
        b.iter(|| {
            sample_bilinear(&img, test::black_box(0.5), test::black_box(0.5));
        });
    }
    #[test]
    fn test_sample_nearest_correctness() {
        let img = ImageBuffer::from_fn(2, 2, |x, y| match (x, y) {
            (0, 0) => Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 0,
            },
            (0, 1) => Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 0,
            },
            (1, 0) => Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 0,
            },
            (1, 1) => Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            _ => panic!(),
        });

        assert_eq!(
            sample_nearest(&img, 0.0, 0.0),
            Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 0
            })
        );
        assert_eq!(
            sample_nearest(&img, 0.0, 1.0),
            Some(Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 0
            })
        );
        assert_eq!(
            sample_nearest(&img, 1.0, 0.0),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 0
            })
        );
        assert_eq!(
            sample_nearest(&img, 1.0, 1.0),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255
            })
        );

        assert_eq!(
            sample_nearest(&img, 0.5, 0.5),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255
            })
        );
        assert_eq!(
            sample_nearest(&img, 0.5, 0.0),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 0
            })
        );
        assert_eq!(
            sample_nearest(&img, 0.0, 0.5),
            Some(Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 0
            })
        );
        assert_eq!(
            sample_nearest(&img, 0.5, 1.0),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255
            })
        );
        assert_eq!(
            sample_nearest(&img, 1.0, 0.5),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255
            })
        );
    }

    #[bench]
    #[cfg(all(feature = "benchmarks", feature = "tiff"))]
    fn bench_resize_same_size(b: &mut test::Bencher) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/images/tiff/testsuite/mandrill.tiff"
        );
        let image = crate::open(path).unwrap();
        b.iter(|| {
            test::black_box(image.resize(image.width(), image.height(), FilterType::CatmullRom));
        });
        b.bytes = (image.width() * image.height() * 3) as u64;
    }

    #[test]
    fn test_issue_186() {
        let img: RgbImage = ImageBuffer::new(100, 100);
        let _ = resize(&img, 50, 50, FilterType::Lanczos3);
    }

    #[bench]
    #[cfg(all(feature = "benchmarks", feature = "tiff"))]
    fn bench_thumbnail(b: &mut test::Bencher) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/images/tiff/testsuite/mandrill.tiff"
        );
        let image = crate::open(path).unwrap();
        b.iter(|| {
            test::black_box(image.thumbnail(256, 256));
        });
        b.bytes = 512 * 512 * 4 + 256 * 256 * 4;
    }

    #[bench]
    #[cfg(all(feature = "benchmarks", feature = "tiff"))]
    fn bench_thumbnail_upsize(b: &mut test::Bencher) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/images/tiff/testsuite/mandrill.tiff"
        );
        let image = crate::open(path).unwrap().thumbnail(256, 256);
        b.iter(|| {
            test::black_box(image.thumbnail(512, 512));
        });
        b.bytes = 512 * 512 * 4 + 256 * 256 * 4;
    }

    #[bench]
    #[cfg(all(feature = "benchmarks", feature = "tiff"))]
    fn bench_thumbnail_upsize_irregular(b: &mut test::Bencher) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/images/tiff/testsuite/mandrill.tiff"
        );
        let image = crate::open(path).unwrap().thumbnail(193, 193);
        b.iter(|| {
            test::black_box(image.thumbnail(256, 256));
        });
        b.bytes = 193 * 193 * 4 + 256 * 256 * 4;
    }

    #[test]
    #[cfg(feature = "png")]
    fn resize_transparent_image() {
        use super::FilterType::{CatmullRom, Gaussian, Lanczos3, Nearest, Triangle};
        use crate::imageops::crop_imm;
        use crate::RgbaImage;

        fn assert_resize(image: &RgbaImage, filter: FilterType) {
            let resized = resize(image, 16, 16, filter);
            let cropped = crop_imm(&resized, 5, 5, 6, 6).to_image();
            for pixel in cropped.pixels() {
                assert!(
                    pixel.a != 254 && pixel.a != 253,
                    "alpha value: {}, {:?}",
                    pixel.a,
                    filter
                );
            }
        }

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/images/png/transparency/tp1n3p08.png"
        );
        let img = crate::open(path).unwrap();
        let rgba8 = img.as_rgba8().unwrap();
        let filters = &[Nearest, Triangle, CatmullRom, Gaussian, Lanczos3];
        for filter in filters {
            assert_resize(rgba8, *filter);
        }
    }

    #[test]
    fn bug_1600() {
        let image = crate::RgbaImage::from_raw(629, 627, vec![255; 629 * 627 * 4]).unwrap();
        let result = resize(&image, 22, 22, FilterType::Lanczos3);
        assert!(result.into_raw().into_iter().any(|c| c != 0));
    }
}
