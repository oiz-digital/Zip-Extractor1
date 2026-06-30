import React from "react";
import { Link } from "wouter";
import { useGetValidators, useGetValidatorStats } from "@workspace/api-client-react";
import { formatNumber, formatAddress } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { Users, ShieldCheck, Activity } from "lucide-react";

export default function Validators() {
  const { data: validators, isLoading: validatorsLoading } = useGetValidators();
  const { data: stats, isLoading: statsLoading } = useGetValidatorStats();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">Validators & Staking</h2>
        <p className="text-muted-foreground">Network validators, staking distribution, and security metrics.</p>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Users className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Active Validators</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-16" /> : `${stats?.activeValidators} / ${stats?.totalValidators}`}
              </h3>
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><ShieldCheck className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Total Staked</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatNumber(stats?.totalStaked || 0, 0)}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Activity className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">APR</p>
              <h3 className="text-xl font-bold font-mono text-green-500">
                {statsLoading ? <Skeleton className="h-7 w-16" /> : `${formatNumber(stats?.annualizedReward || 0, 2)}%`}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><ShieldCheck className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Bonded Ratio</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-16" /> : `${formatNumber(stats?.bondedRatio || 0, 2)}%`}
              </h3>
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardContent className="p-0">
          {validatorsLoading ? (
            <div className="p-4 space-y-2">
              {Array.from({ length: 10 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-16">Rank</TableHead>
                  <TableHead>Moniker / Address</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead className="text-right">Voting Power</TableHead>
                  <TableHead className="text-right">Commission</TableHead>
                  <TableHead className="text-right">Uptime</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {validators?.map((validator, index) => (
                  <TableRow key={validator.address}>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      {validator.rank || index + 1}
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-col">
                        <Link href={`/validators/${validator.address}`} className="font-semibold text-primary hover:underline">
                          {validator.moniker || 'Unknown'}
                        </Link>
                        <span className="text-[10px] font-mono text-muted-foreground">{formatAddress(validator.address, 10)}</span>
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge variant={validator.status === 'active' ? 'default' : 'secondary'} className="text-[10px] uppercase">
                        {validator.status}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-right font-mono text-sm">
                      {formatNumber(validator.votingPower, 0)}
                    </TableCell>
                    <TableCell className="text-right font-mono text-sm">
                      {formatNumber(validator.commission * 100, 1)}%
                    </TableCell>
                    <TableCell className="text-right font-mono text-sm">
                      <span className={validator.uptime > 99 ? 'text-green-500' : validator.uptime > 95 ? 'text-yellow-500' : 'text-destructive'}>
                        {formatNumber(validator.uptime, 2)}%
                      </span>
                    </TableCell>
                  </TableRow>
                ))}
                {validators?.length === 0 && (
                  <TableRow>
                    <TableCell colSpan={6} className="text-center py-8 text-muted-foreground">
                      No validators found.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
