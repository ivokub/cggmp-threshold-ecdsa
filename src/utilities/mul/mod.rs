#![allow(non_snake_case)]
/*
	CGGMP Threshold ECDSA

	Copyright 2022 by Webb Technologies

	This file is part of CGGMP Threshold ECDSA library
	(https://github.com/webb-tools/cggmp-threshold-ecdsa)

	CGGMP Threshold ECDSA is free software: you can redistribute
	it and/or modify it under the terms of the GNU General Public
	License as published by the Free Software Foundation, either
	version 3 of the License, or (at your option) any later version.

	@license GPL-3.0+ <https://github.com/KZen-networks/multi-party-ecdsa/blob/master/LICENSE>
*/

use super::sample_relatively_prime_integer;
use crate::{utilities::mod_pow_with_negative, Error};
use curv::{
	arithmetic::{traits::*, Modulo},
	cryptographic_primitives::hashing::{Digest, DigestExt},
	elliptic::curves::Curve,
	BigInt,
};
use paillier::{EncryptWithChosenRandomness, EncryptionKey, Paillier, Randomness, RawPlaintext};
use rand::Rng;
use rand_chacha::{rand_core::SeedableRng, ChaChaRng};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaillierMulStatement<E: Curve, H: Digest + Clone> {
	pub N: BigInt,
	pub NN: BigInt,
	pub C: BigInt,
	pub Y: BigInt,
	pub X: BigInt,
	pub ek_prover: EncryptionKey,
	pub phantom: PhantomData<(E, H)>,
}

pub struct PaillierMulWitness<E: Curve, H: Digest + Clone> {
	x: BigInt,
	rho: BigInt,
	rho_x: BigInt,
	phantom: PhantomData<(E, H)>,
}

impl<E: Curve, H: Digest + Clone> PaillierMulWitness<E, H> {
	pub fn new(x: BigInt, rho: BigInt, rho_x: BigInt) -> Self {
		PaillierMulWitness { x, rho, rho_x, phantom: PhantomData }
	}
}

impl<E: Curve, H: Digest + Clone> PaillierMulStatement<E, H> {
	#[allow(clippy::too_many_arguments)]
	pub fn generate(
		rho: BigInt,
		rho_x: BigInt,
		prover: EncryptionKey,
		Y: BigInt,
	) -> (Self, PaillierMulWitness<E, H>) {
		let ek_prover = prover.clone();
		// x <- Z_N
		let x = BigInt::sample_below(&prover.n);
		// X = (1 + N)^x * rho_x^N mod N^2
		let X = Paillier::encrypt_with_chosen_randomness(
			&ek_prover,
			RawPlaintext::from(x.clone()),
			&Randomness::from(rho_x.clone()),
		);
		// C = Y^x * rho^N mod N^2
		let C = BigInt::mod_mul(
			&BigInt::mod_pow(&Y, &x, &prover.nn),
			&BigInt::mod_pow(&rho, &prover.n, &prover.nn),
			&prover.nn,
		);

		(
			Self {
				N: prover.n,
				NN: prover.nn,
				C,
				Y,
				X: X.clone().into(),
				ek_prover,
				phantom: PhantomData,
			},
			PaillierMulWitness { x, rho, rho_x, phantom: PhantomData },
		)
	}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaillierMulCommitment {
	A: BigInt,
	B: BigInt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaillierMulProof<E: Curve, H: Digest + Clone> {
	z: BigInt,
	u: BigInt,
	v: BigInt,
	commitment: PaillierMulCommitment,
	phantom: PhantomData<(E, H)>,
}

// Link to the UC non-interactive threshold ECDSA paper
impl<E: Curve, H: Digest + Clone> PaillierMulProof<E, H> {
	pub fn prove(
		witness: &PaillierMulWitness<E, H>,
		statement: &PaillierMulStatement<E, H>,
	) -> PaillierMulProof<E, H> {
		// α,r,s <- Z∗_N
		let alpha = sample_relatively_prime_integer(&statement.N);
		let r = sample_relatively_prime_integer(&statement.N);
		let s = sample_relatively_prime_integer(&statement.N);
		// A = Y^α * r^N mod N^2
		let A = BigInt::mod_mul(
			&mod_pow_with_negative(&statement.Y, &alpha, &statement.NN),
			&BigInt::mod_pow(&r, &statement.N, &statement.NN),
			&statement.NN,
		);
		// B = (1 + N)^α * s^N mod N^2
		let B = {
			let B_ciphertext = Paillier::encrypt_with_chosen_randomness(
				&statement.ek_prover,
				RawPlaintext::from(alpha.clone()),
				&Randomness::from(s.clone()),
			);
			let B_bigint: BigInt = B_ciphertext.into();
			B_bigint.mod_floor(&statement.NN)
		};
		// e = H(A,B)
		let mut e = H::new().chain_bigint(&A).chain_bigint(&B).result_bigint();
		let mut rng: ChaChaRng = ChaChaRng::from_seed(e.to_bytes().try_into().unwrap());
		let val = rng.gen_range(0..2);
		e = BigInt::from(val).mul(&BigInt::from(-2)).add(&BigInt::one()).mul(&e);
		let commitment: PaillierMulCommitment = PaillierMulCommitment { A, B };
		// z = α + e * x mod N
		let z = BigInt::add(&alpha, &BigInt::mul(&e, &witness.x));
		// u = r * rho^e mod N
		let u = BigInt::mod_mul(
			&r,
			&mod_pow_with_negative(&witness.rho, &e, &statement.N),
			&statement.N,
		);
		// v = s * rho_x^e mod N
		let v = BigInt::mod_mul(
			&s,
			&mod_pow_with_negative(&witness.rho_x, &e, &statement.N),
			&statement.N,
		);
		// Return the proof
		PaillierMulProof { z, u, v, commitment, phantom: PhantomData }
	}

	pub fn verify(
		proof: &PaillierMulProof<E, H>,
		statement: &PaillierMulStatement<E, H>,
	) -> Result<(), Error> {
		// Compute the challenge
		let mut e = H::new()
			.chain_bigint(&proof.commitment.A)
			.chain_bigint(&proof.commitment.B)
			.result_bigint();
		let mut rng: ChaChaRng = ChaChaRng::from_seed(e.to_bytes().try_into().unwrap());
		let val = rng.gen_range(0..2);
		e = BigInt::from(val).mul(&BigInt::from(-2)).add(&BigInt::one()).mul(&e);
		/*
			FIRST EQUALITY CHECK
			Y^z · u^N = A · C^e mod N^2
		*/
		let left_1 = BigInt::mod_mul(
			&mod_pow_with_negative(&statement.Y, &proof.z, &statement.NN),
			&BigInt::mod_pow(&proof.u, &statement.N, &statement.NN),
			&statement.NN,
		);
		let right_1 = BigInt::mod_mul(
			&proof.commitment.A,
			&mod_pow_with_negative(&statement.C, &e, &statement.NN),
			&statement.NN,
		);
		assert!(left_1 == right_1);
		/*
			SECOND EQUALITY CHECK
			(1 + N)^z · v^N = B · X^e mod N^2 === Enc(z,c) = B · X^e mod N^2
		*/
		let left_ciphertext = Paillier::encrypt_with_chosen_randomness(
			&statement.ek_prover,
			RawPlaintext::from(proof.z.clone()),
			&Randomness::from(proof.v.clone()),
		);
		let left_2: BigInt = left_ciphertext.into();
		let right_2 = BigInt::mod_mul(
			&proof.commitment.B,
			&mod_pow_with_negative(&statement.X, &e, &statement.NN),
			&statement.NN,
		);
		assert!(left_2.mod_floor(&statement.NN) == right_2);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::utilities::{mta::range_proofs::SampleFromMultiplicativeGroup, BITS_PAILLIER};
	use curv::elliptic::curves::secp256_k1::Secp256k1;
	use paillier::{Encrypt, KeyGeneration, Paillier, RawPlaintext};
	use sha2::Sha256;

	#[test]
	fn test_paillier_mul() {
		let (ek_prover, _) = Paillier::keypair_with_modulus_size(BITS_PAILLIER).keys();
		let rho: BigInt = BigInt::from_paillier_key(&ek_prover);
		let rho_x: BigInt = BigInt::from_paillier_key(&ek_prover);
		let Y = Paillier::encrypt(&ek_prover, RawPlaintext::from(BigInt::from(12)));
		let (statement, witness) = PaillierMulStatement::<Secp256k1, Sha256>::generate(
			rho,
			rho_x,
			ek_prover,
			Y.0.into_owned(),
		);
		let proof = PaillierMulProof::<Secp256k1, Sha256>::prove(&witness, &statement);
		assert!(PaillierMulProof::<Secp256k1, Sha256>::verify(&proof, &statement).is_ok());
	}
}
