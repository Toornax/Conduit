//! Tampon audio planaire pré-alloué.

use crate::types::Frames;

/// Tampon audio planaire : un `Vec<f32>` par canal, tous de même capacité.
///
/// La construction alloue ; **aucune méthode n'alloue ensuite**. La longueur
/// active (`frames`) peut être réduite ou remise à la capacité sans réallocation,
/// ce qui permet de traiter des cycles plus courts que le quantum.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    channels: Vec<Vec<f32>>,
    capacity: usize,
    frames: usize,
}

impl AudioBuffer {
    /// Alloue `channels` canaux de `capacity` trames, remplis de silence.
    /// La longueur active initiale est `capacity`.
    pub fn new(channels: usize, capacity: usize) -> Self {
        Self {
            channels: (0..channels).map(|_| vec![0.0; capacity]).collect(),
            capacity,
            frames: capacity,
        }
    }

    /// Nombre de canaux.
    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// Capacité en trames (allouée).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Longueur active en trames.
    pub fn frames(&self) -> Frames {
        Frames::new(self.frames)
    }

    /// Longueur active en trames (`usize`).
    pub fn len(&self) -> usize {
        self.frames
    }

    /// Vrai si la longueur active est nulle.
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Fixe la longueur active. Panique si `frames > capacity`.
    ///
    /// Temps réel : oui.
    pub fn set_frames(&mut self, frames: usize) {
        assert!(
            frames <= self.capacity,
            "frames {frames} > capacité {}",
            self.capacity
        );
        self.frames = frames;
    }

    /// Vue immuable sur un canal (longueur active).
    ///
    /// Temps réel : oui.
    #[inline]
    pub fn channel(&self, ch: usize) -> &[f32] {
        &self.channels[ch][..self.frames]
    }

    /// Vue mutable sur un canal (longueur active).
    ///
    /// Temps réel : oui.
    #[inline]
    pub fn channel_mut(&mut self, ch: usize) -> &mut [f32] {
        let n = self.frames;
        &mut self.channels[ch][..n]
    }

    /// Itère sur les canaux (vues de longueur active).
    pub fn iter(&self) -> impl Iterator<Item = &[f32]> + '_ {
        self.channels.iter().map(move |c| &c[..self.frames])
    }

    /// Itère mutablement sur les canaux.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut [f32]> + '_ {
        let n = self.frames;
        self.channels.iter_mut().map(move |c| &mut c[..n])
    }

    /// Remplit tous les canaux de zéros (longueur active).
    ///
    /// Temps réel : oui.
    pub fn fill_silence(&mut self) {
        for c in self.iter_mut() {
            c.fill(0.0);
        }
    }

    /// Copie `src` dans `self`, canal par canal, sur `min(len)` trames et
    /// `min(channels)` canaux. Les canaux de `self` sans correspondance sont mis
    /// au silence. Ne modifie pas la longueur active.
    ///
    /// Temps réel : oui.
    pub fn copy_from(&mut self, src: &AudioBuffer) {
        let n = self.frames.min(src.frames);
        for (ch, dst) in self.channels.iter_mut().enumerate() {
            let dst = &mut dst[..self.frames];
            match src.channels.get(ch) {
                Some(s) => {
                    dst[..n].copy_from_slice(&s[..n]);
                    dst[n..].fill(0.0);
                }
                None => dst.fill(0.0),
            }
        }
    }

    /// Ajoute `src * gain` à `self` canal par canal (mixage), sur `min(len)` trames.
    ///
    /// Temps réel : oui.
    pub fn add_from(&mut self, src: &AudioBuffer, gain: f32) {
        let n = self.frames.min(src.frames);
        for (dst, s) in self.channels.iter_mut().zip(src.channels.iter()) {
            for (d, x) in dst[..n].iter_mut().zip(&s[..n]) {
                *d += x * gain;
            }
        }
    }

    /// Multiplie tous les échantillons par `gain`.
    ///
    /// Temps réel : oui.
    pub fn scale(&mut self, gain: f32) {
        for c in self.iter_mut() {
            for x in c {
                *x *= gain;
            }
        }
    }

    /// Crête absolue sur tous les canaux (longueur active).
    ///
    /// Temps réel : oui.
    pub fn peak(&self) -> f32 {
        self.iter()
            .flat_map(|c| c.iter())
            .fold(0.0f32, |m, x| m.max(x.abs()))
    }

    /// Désentrelace `interleaved` (trames × canaux) dans `self`. Le nombre de canaux
    /// de `interleaved` est `self.channels()`. Traite `min(frames)` trames.
    ///
    /// Temps réel : oui.
    pub fn deinterleave_from(&mut self, interleaved: &[f32]) {
        let nch = self.channels.len();
        if nch == 0 {
            return;
        }
        let n = self.frames.min(interleaved.len() / nch);
        for (ch, dst) in self.channels.iter_mut().enumerate() {
            for (i, d) in dst[..n].iter_mut().enumerate() {
                *d = interleaved[i * nch + ch];
            }
        }
    }

    /// Entrelace `self` dans `interleaved` (trames × canaux). Traite `min(frames)` trames.
    ///
    /// Temps réel : oui.
    pub fn interleave_into(&self, interleaved: &mut [f32]) {
        let nch = self.channels.len();
        if nch == 0 {
            return;
        }
        let n = self.frames.min(interleaved.len() / nch);
        for (ch, src) in self.channels.iter().enumerate() {
            for (i, s) in src[..n].iter().enumerate() {
                interleaved[i * nch + ch] = *s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_silent_with_capacity() {
        let b = AudioBuffer::new(2, 64);
        assert_eq!(b.channels(), 2);
        assert_eq!(b.capacity(), 64);
        assert_eq!(b.len(), 64);
        assert!(b.iter().all(|c| c.iter().all(|&x| x == 0.0)));
    }

    #[test]
    fn set_frames_shrinks_views() {
        let mut b = AudioBuffer::new(1, 64);
        b.channel_mut(0).fill(1.0);
        b.set_frames(16);
        assert_eq!(b.channel(0).len(), 16);
        b.fill_silence();
        b.set_frames(64);
        assert_eq!(b.channel(0)[..16].iter().sum::<f32>(), 0.0);
        assert_eq!(b.channel(0)[16..].iter().sum::<f32>(), 48.0);
    }

    #[test]
    #[should_panic(expected = "capacité")]
    fn set_frames_over_capacity_panics() {
        AudioBuffer::new(1, 8).set_frames(9);
    }

    #[test]
    fn copy_from_handles_channel_and_length_mismatch() {
        let mut src = AudioBuffer::new(1, 8);
        src.channel_mut(0).fill(0.5);
        let mut dst = AudioBuffer::new(2, 4);
        dst.channel_mut(1).fill(9.0);
        dst.copy_from(&src);
        assert_eq!(dst.channel(0), &[0.5; 4]);
        assert_eq!(dst.channel(1), &[0.0; 4]);

        let mut short = AudioBuffer::new(1, 2);
        short.channel_mut(0).fill(1.0);
        let mut long = AudioBuffer::new(1, 4);
        long.channel_mut(0).fill(7.0);
        long.copy_from(&short);
        assert_eq!(long.channel(0), &[1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn add_from_and_scale() {
        let mut a = AudioBuffer::new(2, 4);
        a.channel_mut(0).fill(1.0);
        let mut b = AudioBuffer::new(2, 4);
        b.channel_mut(0).fill(2.0);
        b.channel_mut(1).fill(3.0);
        a.add_from(&b, 0.5);
        assert_eq!(a.channel(0), &[2.0; 4]);
        assert_eq!(a.channel(1), &[1.5; 4]);
        a.scale(2.0);
        assert_eq!(a.channel(1), &[3.0; 4]);
        assert_eq!(a.peak(), 4.0);
    }

    #[test]
    fn interleave_roundtrip() {
        let inter = [1.0, 10.0, 2.0, 20.0, 3.0, 30.0];
        let mut b = AudioBuffer::new(2, 3);
        b.deinterleave_from(&inter);
        assert_eq!(b.channel(0), &[1.0, 2.0, 3.0]);
        assert_eq!(b.channel(1), &[10.0, 20.0, 30.0]);
        let mut out = [0.0; 6];
        b.interleave_into(&mut out);
        assert_eq!(out, inter);
    }

    #[test]
    fn zero_channels_is_harmless() {
        let mut b = AudioBuffer::new(0, 8);
        b.fill_silence();
        b.deinterleave_from(&[1.0; 8]);
        let mut out = [0.0; 8];
        b.interleave_into(&mut out);
        assert_eq!(b.peak(), 0.0);
    }
}
