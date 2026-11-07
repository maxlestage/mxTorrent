//! Champ de bits des pièces possédées.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bitfield {
    bytes: Vec<u8>,
    len: usize,
    count: usize,
}

impl Bitfield {
    pub fn new(len: usize) -> Bitfield {
        Bitfield { bytes: vec![0; len.div_ceil(8)], len, count: 0 }
    }

    pub fn full(len: usize) -> Bitfield {
        let mut b = Bitfield::new(len);
        for i in 0..len {
            b.set(i);
        }
        b
    }

    /// Lit un champ reçu d'un pair ; les bits de bourrage sont ignorés.
    pub fn from_bytes(raw: &[u8], len: usize) -> Option<Bitfield> {
        if raw.len() != len.div_ceil(8) {
            return None;
        }
        let mut b = Bitfield::new(len);
        for i in 0..len {
            if raw[i / 8] & (0x80 >> (i % 8)) != 0 {
                b.set(i);
            }
        }
        Some(b)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn is_full(&self) -> bool {
        self.len > 0 && self.count == self.len
    }

    pub fn get(&self, i: usize) -> bool {
        i < self.len && self.bytes[i / 8] & (0x80 >> (i % 8)) != 0
    }

    /// Renvoie vrai si le bit vient de passer à 1.
    pub fn set(&mut self, i: usize) -> bool {
        if i >= self.len || self.get(i) {
            return false;
        }
        self.bytes[i / 8] |= 0x80 >> (i % 8);
        self.count += 1;
        true
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(|&i| self.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits() {
        let mut b = Bitfield::new(10);
        assert!(b.set(0));
        assert!(!b.set(0));
        b.set(9);
        assert_eq!(b.as_bytes(), &[0x80, 0x40]);
        assert_eq!(b.count(), 2);
        let c = Bitfield::from_bytes(&[0x80, 0x7f], 10).unwrap();
        assert_eq!(c.ones().collect::<Vec<_>>(), vec![0, 9]);
        assert!(Bitfield::from_bytes(&[0], 10).is_none());
        assert!(Bitfield::full(10).is_full());
    }
}
