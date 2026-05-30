// #[PerformanceCriticalPath]
#include "kallisto/siphash.hpp"
#include <cstring>

namespace kallisto {

SipHash::SipHash(uint64_t key_part1, uint64_t key_part2) :
	key_part1_(key_part1), key_part2_(key_part2) {}

uint64_t SipHash::hash(const std::string& input) const {
	return hash(input, key_part1_, key_part2_);
}

uint64_t SipHash::hash(const std::string& input, uint64_t key_part1, uint64_t key_part2) {
	// Initialize internal state using "nothing-up-my-sleeve" constants
	uint64_t state0 = 0x736f6d6570736575ULL ^ key_part1;
	uint64_t state1 = 0x646f72616e646f6dULL ^ key_part2;	
	uint64_t state2 = 0x6c7967656e657261ULL ^ key_part1;
	uint64_t state3 = 0x7465646279746573ULL ^ key_part2;

	const uint8_t* data_ptr = reinterpret_cast<const uint8_t*>(input.data());
	size_t input_len = input.length();
	const uint8_t* full_blocks_end = data_ptr + (input_len & ~7);

	// Compress full 64-bit blocks
	for (; data_ptr < full_blocks_end; data_ptr += 8) {
		uint64_t message_word;
		std::memcpy(&message_word, data_ptr, 8);

		state3 ^= message_word;
		performSipRound(state0, state1, state2, state3);
		performSipRound(state0, state1, state2, state3);
		state0 ^= message_word;
	}

	// Pack remainder bytes
	uint64_t remainder_word = 0;
	switch (input_len & 7) {
		case 7: remainder_word |= static_cast<uint64_t>(data_ptr[6]) << 48; [[fallthrough]];
		case 6: remainder_word |= static_cast<uint64_t>(data_ptr[5]) << 40; [[fallthrough]];
		case 5: remainder_word |= static_cast<uint64_t>(data_ptr[4]) << 32; [[fallthrough]];
		case 4: remainder_word |= static_cast<uint64_t>(data_ptr[3]) << 24; [[fallthrough]];
		case 3: remainder_word |= static_cast<uint64_t>(data_ptr[2]) << 16; [[fallthrough]];
		case 2: remainder_word |= static_cast<uint64_t>(data_ptr[1]) << 8;  [[fallthrough]];
		case 1: remainder_word |= static_cast<uint64_t>(data_ptr[0]);       break;
		case 0: break;
	}

	// Compress the final block
	uint64_t final_block = (static_cast<uint64_t>(input_len) << 56) | remainder_word;
	state3 ^= final_block;
	performSipRound(state0, state1, state2, state3);
	performSipRound(state0, state1, state2, state3);
	state0 ^= final_block;

	// Finalization step
	state2 ^= 0xff;
	for (int i = 0; i < 4; ++i) {
		performSipRound(state0, state1, state2, state3);
	}

	return state0 ^ state1 ^ state2 ^ state3;
}
} // namespace kallisto
