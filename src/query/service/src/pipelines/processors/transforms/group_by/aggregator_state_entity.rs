// Copyright 2021 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use common_hashtable::HashtableEntry;
use common_hashtable::HashtableKeyable;
use common_hashtable::UnsizedHashtableFakeEntry;

pub trait StateEntity {
    type KeyRef: Copy;

    fn get_state_key(self: *mut Self) -> Self::KeyRef;
    fn set_state_value(self: *mut Self, value: usize);
    fn get_state_value<'a>(self: *mut Self) -> &'a usize;
}

pub trait ShortFixedKeyable: Sized + Clone {
    fn lookup(&self) -> isize;
    fn is_zero_key(&self) -> bool;
}

pub struct ShortFixedKeysStateEntity<Key: ShortFixedKeyable> {
    pub key: Key,
    pub value: usize,
    pub fill: bool,
}

impl<Key: ShortFixedKeyable + Copy> StateEntity for ShortFixedKeysStateEntity<Key> {
    type KeyRef = Key;

    #[inline(always)]
    fn get_state_key<'a>(self: *mut Self) -> Key {
        unsafe { (*self).key }
    }

    #[inline(always)]
    fn set_state_value(self: *mut Self, value: usize) {
        unsafe { (*self).value = value }
    }

    #[inline(always)]
    fn get_state_value<'a>(self: *mut Self) -> &'a usize {
        unsafe { &(*self).value }
    }
}

impl<Key: HashtableKeyable> StateEntity for HashtableEntry<Key, usize> {
    type KeyRef = Key;

    #[inline(always)]
    fn get_state_key(self: *mut Self) -> Key {
        unsafe { *(*self).key() }
    }

    #[inline(always)]
    fn set_state_value(self: *mut Self, value: usize) {
        unsafe {
            *(*self).get_mut() = value;
        }
    }

    #[inline(always)]
    fn get_state_value<'a>(self: *mut Self) -> &'a usize {
        unsafe { &*((*self).get() as *const _) }
    }
}

impl StateEntity for UnsizedHashtableFakeEntry<[u8], usize> {
    type KeyRef = *const [u8];

    #[inline(always)]
    fn get_state_key(self: *mut Self) -> *const [u8] {
        unsafe { self.key() }
    }

    #[inline(always)]
    fn set_state_value(self: *mut Self, value: usize) {
        unsafe {
            *self.get_mut() = value;
        }
    }

    #[inline(always)]
    fn get_state_value<'a>(self: *mut Self) -> &'a usize {
        unsafe { self.get() }
    }
}

impl ShortFixedKeyable for u8 {
    #[inline(always)]
    fn lookup(&self) -> isize {
        *self as isize
    }

    #[inline(always)]
    fn is_zero_key(&self) -> bool {
        *self == 0
    }
}

impl ShortFixedKeyable for u16 {
    #[inline(always)]
    fn lookup(&self) -> isize {
        *self as isize
    }

    #[inline(always)]
    fn is_zero_key(&self) -> bool {
        *self == 0
    }
}
